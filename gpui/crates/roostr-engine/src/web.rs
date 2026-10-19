//! The browser engine: the site's `engine.wasm`, verified against its
//! `engine-core.json` (`core.ts initCore`), instantiated with the headless
//! Odin runtime of `odin-runtime.ts` ported to Rust closures.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Function, Object, Reflect, Uint8Array, WebAssembly};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::{ABI_VERSION, EngineError};

/// Where the engine module and its manifest are served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSource {
    /// `engine.wasm`; fetched pinned to the manifest's hash (`?v=…`).
    pub wasm_url: String,
    /// `engine-core.json` from glonOdin `build-wasm.mjs`; fetched uncached.
    pub manifest_url: String,
}

impl WebSource {
    /// `engine.wasm` and `engine-core.json` under `base` (e.g. `"/"`).
    pub fn under(base: &str) -> WebSource {
        let base = base.trim_end_matches('/');
        WebSource {
            wasm_url: format!("{base}/engine.wasm"),
            manifest_url: format!("{base}/engine-core.json"),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    abi_version: u32,
    sha256: String,
    source_fingerprint: String,
}

/// Odin's diagnostics (`odin_env.write`), kept for the trap message.
const DIAGNOSTIC_LIMIT: usize = 8192;

/// Host state the import closures share with the engine handle.
#[derive(Default)]
struct Runtime {
    memory: RefCell<Option<WebAssembly::Memory>>,
    diagnostic: RefCell<String>,
}

impl Runtime {
    fn view(&self, pointer: u32, length: u32) -> Result<Uint8Array, JsValue> {
        let memory = self.memory.borrow();
        let memory = memory
            .as_ref()
            .ok_or_else(|| JsValue::from(js_sys::Error::new("Odin memory is not initialized")))?;
        Ok(Uint8Array::new_with_byte_offset_and_length(
            &memory.buffer(),
            pointer,
            length,
        ))
    }

    fn trap(&self) -> JsValue {
        let diagnostic = self.diagnostic.borrow();
        let message = diagnostic.trim();
        js_sys::Error::new(if message.is_empty() {
            "Odin core trapped"
        } else {
            message
        })
        .into()
    }
}

struct Exports {
    memory: WebAssembly::Memory,
    reserve: Function,
    reserve_blob: Function,
    execute: Function,
    response_pointer: Function,
    response_length: Function,
    reset: Function,
}

pub(crate) struct Host {
    exports: Exports,
    runtime: Rc<Runtime>,
    fingerprint: String,
    generation: Cell<u64>,
    active: Cell<bool>,
    /// The import closures live as long as the instance that calls them.
    _imports: Vec<Box<dyn std::any::Any>>,
}

fn load_error(context: &str, error: JsValue) -> EngineError {
    let detail = error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"));
    EngineError::Load(format!("{context}: {detail}"))
}

fn runtime_error(error: JsValue) -> EngineError {
    match load_error("trap", error) {
        EngineError::Load(message) => EngineError::Runtime(message),
        other => other,
    }
}

/// `fetch` on the window or the worker global.
async fn fetch(url: &str, cache: web_sys::RequestCache) -> Result<web_sys::Response, EngineError> {
    let init = web_sys::RequestInit::new();
    init.set_cache(cache);
    let request =
        web_sys::Request::new_with_str_and_init(url, &init).map_err(|e| load_error(url, e))?;
    let global = js_sys::global();
    let promise = if let Some(window) = global.dyn_ref::<web_sys::Window>() {
        window.fetch_with_request(&request)
    } else if let Some(worker) = global.dyn_ref::<web_sys::WorkerGlobalScope>() {
        worker.fetch_with_request(&request)
    } else {
        return Err(EngineError::Load(
            "no fetch in this JavaScript global".into(),
        ));
    };
    let response: web_sys::Response = JsFuture::from(promise)
        .await
        .map_err(|e| load_error(url, e))?
        .unchecked_into();
    if !response.ok() {
        return Err(EngineError::Load(format!(
            "{url}: HTTP {}",
            response.status()
        )));
    }
    Ok(response)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn get(target: &JsValue, name: &str) -> Result<JsValue, EngineError> {
    Reflect::get(target, &JsValue::from_str(name)).map_err(|e| load_error(name, e))
}

fn function(exports: &JsValue, name: &str) -> Result<Function, EngineError> {
    get(exports, name)?
        .dyn_into::<Function>()
        .map_err(|_| EngineError::Load(format!("engine.wasm is missing {name}")))
}

/// A JS number result as the unsigned 32-bit value it encodes (`>>> 0`).
fn unsigned(value: &JsValue) -> u32 {
    value.as_f64().map_or(0, |number| number as i64 as u32)
}

impl Host {
    pub(crate) async fn load(source: &WebSource) -> Result<Host, EngineError> {
        let manifest_response = fetch(&source.manifest_url, web_sys::RequestCache::NoCache).await?;
        let manifest_text = JsFuture::from(
            manifest_response
                .text()
                .map_err(|e| load_error("manifest", e))?,
        )
        .await
        .map_err(|e| load_error("manifest", e))?
        .as_string()
        .unwrap_or_default();
        let manifest: Manifest = serde_json::from_str(&manifest_text)
            .map_err(|error| EngineError::Load(format!("{}: {error}", source.manifest_url)))?;
        if manifest.abi_version != ABI_VERSION {
            return Err(EngineError::AbiMismatch(manifest.abi_version));
        }

        // The manifest is uncached but engine.wasm sits at a stable URL: pin
        // the request to the manifest's build and refuse a stale module.
        let separator = if source.wasm_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let pin = manifest.sha256.get(..16).unwrap_or(&manifest.sha256);
        let wasm_url = format!("{}{separator}v={pin}", source.wasm_url);
        let wasm_response = fetch(&wasm_url, web_sys::RequestCache::Default).await?;
        let buffer = JsFuture::from(
            wasm_response
                .array_buffer()
                .map_err(|e| load_error(&wasm_url, e))?,
        )
        .await
        .map_err(|e| load_error(&wasm_url, e))?;
        if hex(&Sha256::digest(Uint8Array::new(&buffer).to_vec())) != manifest.sha256 {
            return Err(EngineError::Load(
                "engine.wasm does not match engine-core.json: the browser served a stale engine. Reload the page.".into(),
            ));
        }

        let runtime = Rc::new(Runtime::default());
        let (imports, closures) = odin_imports(&runtime)?;
        let module = JsFuture::from(WebAssembly::compile(&buffer))
            .await
            .map_err(|e| load_error("compile engine.wasm", e))?;
        let instance: WebAssembly::Instance = JsFuture::from(WebAssembly::instantiate_module(
            module.unchecked_ref(),
            &imports,
        ))
        .await
        .map_err(|e| load_error("instantiate engine.wasm", e))?
        .unchecked_into();
        let api: JsValue = instance.exports().into();
        let memory = get(&api, "memory")?
            .dyn_into::<WebAssembly::Memory>()
            .map_err(|_| EngineError::Load("engine.wasm does not export memory".into()))?;
        let start = function(&api, "_start")?;
        let abi_version = function(&api, "core_abi_version")?;
        let exports = Exports {
            memory: memory.clone(),
            reserve: function(&api, "core_reserve")?,
            reserve_blob: function(&api, "core_reserve_blob")?,
            execute: function(&api, "core_execute")?,
            response_pointer: function(&api, "core_response_pointer")?,
            response_length: function(&api, "core_response_length")?,
            reset: function(&api, "core_reset")?,
        };
        *runtime.memory.borrow_mut() = Some(memory);
        start
            .call0(&JsValue::NULL)
            .map_err(|e| load_error("_start", e))?;
        let abi = unsigned(
            &abi_version
                .call0(&JsValue::NULL)
                .map_err(|e| load_error("core_abi_version", e))?,
        );
        if abi != ABI_VERSION {
            return Err(EngineError::AbiMismatch(abi));
        }
        Ok(Host {
            exports,
            runtime,
            fingerprint: manifest.source_fingerprint,
            generation: Cell::new(0),
            active: Cell::new(false),
            _imports: closures,
        })
    }

    pub(crate) fn source_fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub(crate) fn reset_all(&self) {
        // Between calls the reset cannot trap; a failure here would be the
        // module's, and the next call reports it.
        let _ = self.exports.reset.call1(&JsValue::NULL, &JsValue::from(1));
        self.generation.set(self.generation.get() + 1);
    }

    fn write(&self, pointer: u32, bytes: &[u8]) {
        Uint8Array::new_with_byte_offset_and_length(
            &self.exports.memory.buffer(),
            pointer,
            bytes.len() as u32,
        )
        .copy_from(bytes);
    }

    fn call(&self, function: &Function, argument: Option<u32>) -> Result<JsValue, EngineError> {
        match argument {
            Some(value) => function.call1(&JsValue::NULL, &JsValue::from(value)),
            None => function.call0(&JsValue::NULL),
        }
        .map_err(runtime_error)
    }

    /// One single-flight request (`core.ts call`). The response is copied out
    /// of linear memory before the reset; a trap resets the whole engine and
    /// bumps the generation.
    pub(crate) fn execute<R>(
        &self,
        request: &[u8],
        blob: Option<&[u8]>,
        decode: impl FnOnce(&[u8]) -> Result<R, EngineError>,
    ) -> Result<R, EngineError> {
        if self.active.replace(true) {
            return Err(EngineError::Runtime(
                "engine calls are not reentrant".into(),
            ));
        }
        self.runtime.diagnostic.borrow_mut().clear();
        let result = self
            .run(request, blob)
            .and_then(|response| decode(&response));
        let fault = matches!(result, Err(EngineError::Runtime(_)));
        let _ = self
            .exports
            .reset
            .call1(&JsValue::NULL, &JsValue::from(u32::from(fault)));
        if fault {
            self.generation.set(self.generation.get() + 1);
        }
        self.active.set(false);
        result
    }

    fn run(&self, request: &[u8], blob: Option<&[u8]>) -> Result<Vec<u8>, EngineError> {
        let pointer = unsigned(&self.call(&self.exports.reserve, Some(request.len() as u32))?);
        if pointer == 0 {
            return Err(EngineError::Runtime(
                "engine refused the request reservation".into(),
            ));
        }
        self.write(pointer, request);
        if let Some(bytes) = blob {
            let pointer =
                unsigned(&self.call(&self.exports.reserve_blob, Some(bytes.len() as u32))?);
            if pointer == 0 {
                return Err(EngineError::BlobTooLarge(bytes.len()));
            }
            self.write(pointer, bytes);
        }
        if unsigned(&self.call(&self.exports.execute, None)?) != 0 {
            return Err(EngineError::Runtime(
                "engine rejected the request state".into(),
            ));
        }
        let output = unsigned(&self.call(&self.exports.response_pointer, None)?);
        let length = unsigned(&self.call(&self.exports.response_length, None)?);
        let buffer = self.exports.memory.buffer();
        let size = buffer.unchecked_ref::<js_sys::ArrayBuffer>().byte_length();
        if length == 0 || u64::from(output) + u64::from(length) > u64::from(size) {
            return Err(EngineError::Runtime(
                "invalid engine response bounds".into(),
            ));
        }
        Ok(Uint8Array::new_with_byte_offset_and_length(&buffer, output, length).to_vec())
    }
}

/// `odin-runtime.ts`: the headless `odin_env` imports. No DOM, eval,
/// filesystem or network capability is supplied to the core.
fn odin_imports(
    runtime: &Rc<Runtime>,
) -> Result<(Object, Vec<Box<dyn std::any::Any>>), EngineError> {
    let env = Object::new();
    let mut closures: Vec<Box<dyn std::any::Any>> = Vec::new();
    let set = |name: &str, value: &JsValue| {
        Reflect::set(&env, &JsValue::from_str(name), value).map_err(|e| load_error(name, e))
    };

    let host = runtime.clone();
    let write = Closure::<dyn Fn(u32, u32, u32) -> Result<(), JsValue>>::new(
        move |_fd: u32, pointer: u32, length: u32| {
            let bytes = host.view(pointer, length)?.to_vec();
            let mut diagnostic = host.diagnostic.borrow_mut();
            diagnostic.push_str(&String::from_utf8_lossy(&bytes));
            if diagnostic.len() > DIAGNOSTIC_LIMIT {
                let mut cut = diagnostic.len() - DIAGNOSTIC_LIMIT;
                while !diagnostic.is_char_boundary(cut) {
                    cut += 1;
                }
                diagnostic.drain(..cut);
            }
            Ok(())
        },
    );
    set("write", write.as_ref())?;
    closures.push(Box::new(write));

    for name in ["trap", "abort"] {
        let host = runtime.clone();
        let trap = Closure::<dyn Fn() -> Result<(), JsValue>>::new(move || Err(host.trap()));
        set(name, trap.as_ref())?;
        closures.push(Box::new(trap));
    }

    let time_now = Closure::<dyn Fn() -> i64>::new(|| js_sys::Date::now() as i64);
    set("time_now", time_now.as_ref())?;
    closures.push(Box::new(time_now));

    let global = js_sys::global();
    let performance = get(&global, "performance")?;
    let now = function(&performance, "now")?.bind0(&performance);
    set("tick_now", &now)?;

    let host = runtime.clone();
    let crypto: web_sys::Crypto = get(&global, "crypto")?.unchecked_into();
    let rand_bytes = Closure::<dyn Fn(u32, u32) -> Result<(), JsValue>>::new(
        move |pointer: u32, length: u32| {
            let output = host.view(pointer, length)?;
            // getRandomValues fills at most 65536 bytes per call.
            let mut offset = 0;
            while offset < length {
                let end = length.min(offset + 65536);
                crypto.get_random_values_with_array_buffer_view(&output.subarray(offset, end))?;
                offset = end;
            }
            Ok(())
        },
    );
    set("rand_bytes", rand_bytes.as_ref())?;
    closures.push(Box::new(rand_bytes));

    let math = get(&global, "Math")?;
    for (import, name) in [
        ("sqrt", "sqrt"),
        ("sin", "sin"),
        ("cos", "cos"),
        ("pow", "pow"),
        ("ln", "log"),
        ("exp", "exp"),
    ] {
        set(import, &get(&math, name)?)?;
    }
    let fmuladd = Closure::<dyn Fn(f64, f64, f64) -> f64>::new(|x: f64, y: f64, z: f64| x * y + z);
    set("fmuladd", fmuladd.as_ref())?;
    closures.push(Box::new(fmuladd));
    let ldexp = Closure::<dyn Fn(f64, f64) -> f64>::new(|x: f64, exponent: f64| {
        x * js_sys::Math::pow(2.0, exponent)
    });
    set("ldexp", ldexp.as_ref())?;
    closures.push(Box::new(ldexp));

    let imports = Object::new();
    Reflect::set(&imports, &JsValue::from_str("odin_env"), &env)
        .map_err(|e| load_error("odin_env", e))?;
    Ok((imports, closures))
}

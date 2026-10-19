//! Linter for a Tool's code. The harness runs a Tool object's Code blocks,
//! joined in reading order, as the body of
//! `export default async function (input, roostr) { … }` and refuses code
//! that doesn't compile. This parses the same wrapped module with oxc and
//! reports, per Code block:
//!  - errors: syntax and grammar errors, redeclared block-scoped names -
//!    code the harness won't run;
//!  - warnings: unused locals, unreachable code, unused labels.
//!
//! Type checking is out of scope (no lib or SDK types are loaded). The unused,
//! unreachable and redeclaration checks follow the TypeScript compiler's rules
//! (`noUnusedLocals`, `allowUnreachableCode: false`, `allowUnusedLabels: false`)
//! and use its message texts.

use std::collections::HashSet;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_ast::ast::{
    ArrayPattern, BindingIdentifier, Declaration, ImportDeclaration, ImportDeclarationSpecifier,
    MethodDefinition, ModuleDeclaration, TSImportEqualsDeclaration,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, BindingPattern, BreakStatement, Class,
    ClassElement, ClassType, ComputedMemberExpression, ContinueStatement, DoWhileStatement,
    Expression, ExpressionStatement, ForInStatement, ForOfStatement, ForStatement,
    ForStatementLeft, Function, FunctionType, IfStatement, LabeledStatement, LogicalOperator,
    MethodDefinitionKind, ObjectPattern, PrivateFieldExpression, PrivateInExpression, PropertyKey,
    ReturnStatement, Statement, StaticBlock, StaticMemberExpression, SwitchStatement,
    TSAccessibility, TSEnumDeclaration, TSInterfaceDeclaration, TSType, TSTypeAliasDeclaration,
    ThrowStatement, TryStatement, UnaryOperator, VariableDeclaration, VariableDeclarationKind,
    WhileStatement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::operator::AssignmentOperator;
use oxc_syntax::scope::{ScopeFlags, ScopeId};
use oxc_syntax::symbol::SymbolFlags;

/// How bad a [`LintProblem`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Code the harness won't run.
    Error,
    /// Code that runs but is probably a mistake.
    Warning,
}

/// One problem in one Code block; offsets are UTF-8 BYTE offsets into that block's text, `to > from` whenever the block has text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LintProblem {
    /// Start offset (bytes) within the block's text.
    pub from: usize,
    /// End offset (bytes, exclusive) within the block's text.
    pub to: usize,
    /// 1-based within the block
    pub line: usize,
    /// 1-based, in chars
    pub col: usize,
    /// Human-readable description.
    pub message: String,
    /// Error or warning.
    pub severity: Severity,
}

const HEADER: &str =
    "export default async function (input: Record<string, unknown>, roostr: Roostr) {\n";

/// TypeScript diagnostic codes of the problems produced here (used to order ties like tsc does).
const TS_REDECLARE: u32 = 2451;
const TS_UNUSED: u32 = 6133;
const TS_UNUSED_TYPE: u32 = 6196;
const TS_UNUSED_DESTRUCTURE: u32 = 6198;
const TS_UNUSED_VARIABLES: u32 = 6199;
const TS_UNREACHABLE: u32 = 7027;
const TS_UNUSED_LABEL: u32 = 7028;
/// Checker diagnostics that hold without lib types (oxc tags some grammar errors with TS codes).
const CHECKER_CODES: [u32; 8] = [2451, 6133, 6196, 6198, 6199, 6205, 7027, 7028];
const MODIFIERS_HERE: &str = "Modifiers cannot appear here.";
const IMPORT_NOT_TOP_LEVEL: &str =
    "An import declaration can only be used at the top level of a namespace or module.";
const EXPORT_NOT_TOP_LEVEL: &str =
    "An export declaration can only be used at the top level of a namespace or module.";

/// A problem in the wrapped source, before mapping it to a block.
struct Found {
    start: usize,
    len: usize,
    code: u32,
    message: String,
    severity: Severity,
}

impl Found {
    fn at(span: Span, code: u32, message: String, severity: Severity) -> Self {
        Self {
            start: span.start as usize,
            len: span.size() as usize,
            code,
            message,
            severity,
        }
    }
}

/// Problems in each of a Tool's Code blocks (texts in reading order), index for index. Synchronous (callers run it on a background thread).
pub fn lint_tool_code(blocks: &[String]) -> Vec<Vec<LintProblem>> {
    let mut out: Vec<Vec<LintProblem>> = vec![Vec::new(); blocks.len()];
    if !blocks.iter().any(|b| !b.trim().is_empty()) {
        return out;
    }
    let code = blocks.join("\n");
    let source = format!("{HEADER}{code}\n}}\n");
    let found = diagnose(&source);

    // Block i's text starts at starts[i] within `code`.
    let mut starts = Vec::with_capacity(blocks.len());
    let mut at = 0;
    for b in blocks {
        starts.push(at);
        at += b.len() + 1;
    }
    for f in found {
        // Anything past the code (an unclosed brace meets the wrapper's end) lands at the end of the last block.
        let rel = f.start.saturating_sub(HEADER.len()).min(code.len());
        let mut i = starts.len() - 1;
        while i > 0 && starts[i] > rel {
            i -= 1;
        }
        let text = blocks[i].as_str();
        let mut from = floor_boundary(text, (rel - starts[i]).min(text.len()));
        let mut to = floor_boundary(text, (from + f.len).min(text.len()));
        if to == from {
            if let Some(c) = text[to..].chars().next() {
                to += c.len_utf8();
            } else if let Some(c) = text[..from].chars().next_back() {
                from -= c.len_utf8();
            }
        }
        let before = &text[..from];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |n| n + 1);
        let col = before[line_start..].chars().count() + 1;
        out[i].push(LintProblem {
            from,
            to,
            line,
            col,
            message: f.message,
            severity: f.severity,
        });
    }
    for list in &mut out {
        list.sort_by_key(|p| p.from);
    }
    out
}

/// The largest char boundary of `text` at or below `at`.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// All problems in the wrapped module: syntax errors, or (when the parse is clean) grammar errors and lint findings.
fn diagnose(source: &str) -> Vec<Found> {
    let allocator = Allocator::default();
    let source_type = SourceType::ts().with_module(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return parsed.diagnostics.iter().map(|d| from_oxc(d, 0)).collect();
    }
    let built = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_build_nodes(true)
        .build(&parsed.program);
    let semantic = &built.semantic;
    let mut found: Vec<Found> = Vec::new();
    for d in built.diagnostics.iter() {
        // Redeclarations are re-derived below with tsc's rules (only block-scoped ones are errors).
        if d.message.starts_with("Identifier `")
            && d.message.ends_with("` has already been declared")
        {
            continue;
        }
        let ts_code = ts_code(d);
        if let Some(n) = ts_code
            && n >= 2000
            && !CHECKER_CODES.contains(&n)
        {
            continue;
        }
        found.push(from_oxc(d, ts_code.unwrap_or(0)));
    }
    let shadowed = redeclarations(semantic, &mut found);
    let mut analyzer = Analyzer::new(semantic, shadowed);
    analyzer.visit_program(&parsed.program);
    analyzer.finish_classes();
    found.append(&mut analyzer.found);
    found.sort_by(|a, b| {
        (a.start, a.len, a.code)
            .cmp(&(b.start, b.len, b.code))
            .then_with(|| a.message.cmp(&b.message))
    });
    found.dedup_by(|a, b| a.start == b.start && a.len == b.len && a.message == b.message);
    found
}

/// The numeric TypeScript code oxc attached to a diagnostic, if any.
fn ts_code(d: &OxcDiagnostic) -> Option<u32> {
    if d.code.scope.as_deref() != Some("TS") {
        return None;
    }
    d.code.number.as_deref()?.parse().ok()
}

fn from_oxc(d: &OxcDiagnostic, code: u32) -> Found {
    let label = d
        .labels
        .iter()
        .find(|l| l.primary())
        .or_else(|| d.labels.first());
    let (start, len) = label.map_or((0, 0), |l| (l.offset() as usize, l.len() as usize));
    Found {
        start,
        len,
        code,
        message: d.message.to_string(),
        severity: Severity::Error,
    }
}

/// tsc's binder conflicts. A declaration clashing with an earlier one in the same symbol table
/// is "Cannot redeclare block-scoped variable" (2451) at every clashing declaration when the
/// earlier one is let/const/using, else "Duplicate identifier" (2300, which tsc callers drop).
/// Returns the clashing (later) declarations' name spans: tsc gives them a detached symbol,
/// so they are never reported unused.
fn redeclarations(semantic: &Semantic<'_>, found: &mut Vec<Found>) -> HashSet<Span> {
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();
    // The tsc symbol table a declaration lands in: `var` hoists to the function, the rest stay put.
    let table = |declaration, flags: SymbolFlags| -> ScopeId {
        let scope = nodes.get_node(declaration).scope_id();
        if flags.contains(SymbolFlags::FunctionScopedVariable) {
            scoping
                .scope_ancestors(scope)
                .find(|&s| scoping.scope_flags(s).is_var())
                .unwrap_or(scope)
        } else {
            scope
        }
    };
    let mut shadowed = HashSet::new();
    for symbol in scoping.symbol_ids() {
        let mut entries: Vec<_> = scoping.symbol_redeclarations(symbol).iter().collect();
        // tsc binds a block's function declarations before its other statements.
        entries.sort_by_key(|e| !e.flags.contains(SymbolFlags::Function));
        let Some((first, rest)) = entries.split_first() else {
            continue;
        };
        let home = table(first.declaration, first.flags);
        let clashing: Vec<Span> = rest
            .iter()
            .filter(|e| {
                first.flags.intersects(excludes(e.flags)) && table(e.declaration, e.flags) == home
            })
            .map(|e| e.span)
            .collect();
        if clashing.is_empty() {
            continue;
        }
        shadowed.extend(clashing.iter().copied());
        if !first.flags.contains(SymbolFlags::BlockScopedVariable) {
            continue;
        }
        let message = format!(
            "Cannot redeclare block-scoped variable '{}'.",
            scoping.symbol_name(symbol)
        );
        for span in std::iter::once(first.span).chain(clashing) {
            found.push(Found::at(
                span,
                TS_REDECLARE,
                message.clone(),
                Severity::Error,
            ));
        }
    }
    shadowed
}

/// The symbol kinds a declaration of this kind cannot share a name with.
fn excludes(flags: SymbolFlags) -> SymbolFlags {
    if flags.contains(SymbolFlags::BlockScopedVariable) {
        SymbolFlags::BlockScopedVariableExcludes
    } else if flags.contains(SymbolFlags::Class) {
        SymbolFlags::ClassExcludes
    } else if flags.contains(SymbolFlags::Function) {
        SymbolFlags::FunctionExcludes
    } else if flags.contains(SymbolFlags::RegularEnum) {
        SymbolFlags::RegularEnumExcludes
    } else if flags.contains(SymbolFlags::ConstEnum) {
        SymbolFlags::ConstEnumExcludes
    } else if flags.contains(SymbolFlags::Interface) {
        SymbolFlags::InterfaceExcludes
    } else if flags.contains(SymbolFlags::TypeAlias) {
        SymbolFlags::TypeAliasExcludes
    } else if flags.contains(SymbolFlags::FunctionScopedVariable) {
        SymbolFlags::FunctionScopedVariableExcludes
    } else if flags.intersects(SymbolFlags::Import | SymbolFlags::TypeImport) {
        SymbolFlags::ImportBindingExcludes
    } else {
        SymbolFlags::empty()
    }
}

/// Flow bit: reachable by control flow and calls (what tsc reports unreachable code from).
const LIVE: u8 = 1;
/// Flow bit: reachable by the binder's syntactic flow (what tsc counts label references in).
const BOUND: u8 = 2;
const REACHABLE: u8 = LIVE | BOUND;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Loop,
    Switch,
    Label,
}

/// A break/continue target on the current function's stack.
struct Target<'a> {
    kind: TargetKind,
    /// Labels naming it: a Label target's own name, or the labels directly wrapping a loop.
    labels: Vec<&'a str>,
    breaks: u8,
    continues: u8,
    referenced: bool,
}

impl<'a> Target<'a> {
    fn new(kind: TargetKind, labels: Vec<&'a str>) -> Self {
        Self {
            kind,
            labels,
            breaks: 0,
            continues: 0,
            referenced: false,
        }
    }
}

/// A `private` or `#private` class member, grouped by name like tsc's member symbols.
struct PrivateMember<'a> {
    name: &'a str,
    hash: bool,
    key: Span,
    /// The function of a method/accessor member (self-calls inside it don't count as reads).
    method: Option<Span>,
    getter: bool,
    setter: bool,
}

struct ClassInfo<'a> {
    span: Span,
    members: Vec<PrivateMember<'a>>,
}

/// A property access that may read a private member.
struct Access<'a> {
    name: &'a str,
    hash: bool,
    span: Span,
    write_only: bool,
    /// `this.x`: tsc ignores it inside x's own method.
    on_this: bool,
    function: Option<Span>,
}

/// One walk over the wrapped module: unreachable code, unused labels, unused locals and private members.
struct Analyzer<'s, 'a> {
    semantic: &'s Semantic<'a>,
    found: Vec<Found>,
    flow: u8,
    /// Inside a statement already reported unreachable: nothing below is reported again.
    suppress: bool,
    targets: Vec<Target<'a>>,
    pending_labels: Vec<&'a str>,
    in_for_in_of_left: bool,
    functions: Vec<Span>,
    classes: Vec<ClassInfo<'a>>,
    accesses: Vec<Access<'a>>,
    write_targets: HashSet<Span>,
    /// Later declarations tsc detached from their clashing name (see [`redeclarations`]).
    shadowed: HashSet<Span>,
    /// Starts of declarations under an `export` (their `declare` is not reported twice).
    exported: HashSet<u32>,
    /// Function spans of class methods (checked with their class, unlike function expressions).
    class_methods: HashSet<Span>,
}

impl<'s, 'a> Analyzer<'s, 'a> {
    fn new(semantic: &'s Semantic<'a>, shadowed: HashSet<Span>) -> Self {
        Self {
            semantic,
            found: Vec::new(),
            flow: REACHABLE,
            suppress: false,
            targets: Vec::new(),
            pending_labels: Vec::new(),
            in_for_in_of_left: false,
            functions: Vec::new(),
            classes: Vec::new(),
            accesses: Vec::new(),
            write_targets: HashSet::new(),
            shadowed,
            exported: HashSet::new(),
            class_methods: HashSet::new(),
        }
    }

    fn warn(&mut self, span: Span, code: u32, message: String) {
        self.found
            .push(Found::at(span, code, message, Severity::Warning));
    }

    fn unreachable(&mut self, span: Span) {
        self.warn(
            span,
            TS_UNREACHABLE,
            "Unreachable code detected.".to_string(),
        );
    }

    /// A grammar error on the `len`-byte keyword starting a statement nested in the wrapper.
    fn keyword_error(&mut self, start: u32, len: u32, message: &str) {
        if !self.functions.is_empty() {
            self.found.push(Found::at(
                Span::sized(start, len),
                0,
                message.to_string(),
                Severity::Error,
            ));
        }
    }

    /// tsc's "Modifiers cannot appear here." for a `declare` statement inside the wrapper.
    fn declare_error(&mut self, declare: bool, span: Span) {
        if declare && !self.exported.contains(&span.start) {
            self.keyword_error(span.start, "declare".len() as u32, MODIFIERS_HERE);
        }
    }

    /// Whether tsc would count the symbol as never read: no reference other than plain
    /// assignments, ignoring references inside its own function/class/type declaration.
    fn is_unused(&self, id: &BindingIdentifier<'a>) -> bool {
        if self.shadowed.contains(&id.span) {
            return false;
        }
        let Some(symbol) = id.symbol_id.get() else {
            return false;
        };
        self.symbol_unused(symbol)
    }

    fn symbol_unused(&self, symbol: SymbolId) -> bool {
        let scoping = self.semantic.scoping();
        let nodes = self.semantic.nodes();
        let own: Vec<Span> = scoping
            .symbol_declarations(symbol)
            .filter_map(|node| match nodes.kind(node) {
                AstKind::Function(f) if f.is_declaration() => Some(f.span),
                AstKind::Class(c) if c.is_declaration() => Some(c.span),
                AstKind::TSInterfaceDeclaration(d) => Some(d.span),
                AstKind::TSTypeAliasDeclaration(d) => Some(d.span),
                AstKind::TSEnumDeclaration(d) => Some(d.span),
                _ => None,
            })
            .collect();
        !scoping.get_resolved_references(symbol).any(|r| {
            if r.flags().is_write_only() {
                return false;
            }
            let span = nodes.get_node(r.node_id()).span();
            !own.iter()
                .any(|d| d.start <= span.start && span.end <= d.end)
        })
    }

    /// tsc's checkUnusedLocalsAndParameters for one variable declaration list.
    fn check_variable_list(&mut self, list: &VariableDeclaration<'a>) {
        let for_in_of = self.in_for_in_of_left;
        let using = matches!(
            list.kind,
            VariableDeclarationKind::Using | VariableDeclarationKind::AwaitUsing
        );
        // Declarators whose whole name is unused: (error span, name).
        let mut unused: Vec<(Span, &'a str)> = Vec::new();
        for declarator in &list.declarations {
            match &declarator.id {
                BindingPattern::BindingIdentifier(id) => {
                    let underscore = id.name.as_str().starts_with('_');
                    if self.is_unused(id) && !((for_in_of || using) && underscore) {
                        unused.push((id.span, id.name.as_str()));
                    }
                }
                BindingPattern::ObjectPattern(pattern) => {
                    if let Some(whole) = self.check_object_pattern(pattern, true) {
                        unused.push(whole);
                    }
                }
                BindingPattern::ArrayPattern(pattern) => self.check_array_pattern(pattern),
                BindingPattern::AssignmentPattern(_) => {}
            }
        }
        if unused.is_empty() {
            return;
        }
        if unused.len() == list.declarations.len() {
            if let [(span, name)] = unused[..] {
                self.warn(span, TS_UNUSED, unused_message(name));
            } else {
                self.warn(
                    list.span,
                    TS_UNUSED_VARIABLES,
                    "All variables are unused.".to_string(),
                );
            }
        } else {
            for (span, name) in unused {
                self.warn(span, TS_UNUSED, unused_message(name));
            }
        }
    }

    /// Unused bindings of an object pattern. When the pattern is a declarator's whole name and
    /// its single element is unused, returns it so the declaration list reports it.
    fn check_object_pattern(
        &mut self,
        pattern: &ObjectPattern<'a>,
        is_declarator_name: bool,
    ) -> Option<(Span, &'a str)> {
        let total = pattern.properties.len() + usize::from(pattern.rest.is_some());
        let has_rest = pattern.rest.is_some();
        let mut unused: Vec<(Span, &'a str)> = Vec::new();
        for property in &pattern.properties {
            match strip_default(&property.value) {
                BindingPattern::BindingIdentifier(id) => {
                    // With a rest element, the other elements just omit properties from it.
                    let exempt = !property.shorthand && id.name.as_str().starts_with('_');
                    if !has_rest && !exempt && self.is_unused(id) {
                        unused.push((id.span, id.name.as_str()));
                    }
                }
                nested => self.check_nested_pattern(nested),
            }
        }
        if let Some(rest) = &pattern.rest {
            match strip_default(&rest.argument) {
                BindingPattern::BindingIdentifier(id) => {
                    if self.is_unused(id) {
                        unused.push((id.span, id.name.as_str()));
                    }
                }
                nested => self.check_nested_pattern(nested),
            }
        }
        if unused.is_empty() {
            return None;
        }
        if unused.len() == total {
            if unused.len() == 1 {
                let name = unused[0].1;
                if is_declarator_name {
                    return Some((pattern.span, name));
                }
                self.warn(pattern.span, TS_UNUSED, unused_message(name));
            } else {
                self.warn(
                    pattern.span,
                    TS_UNUSED_DESTRUCTURE,
                    "All destructured elements are unused.".to_string(),
                );
            }
        } else {
            for (span, name) in unused {
                self.warn(span, TS_UNUSED, unused_message(name));
            }
        }
        None
    }

    /// Array pattern elements are reported one by one (`_`-prefixed names are exempt).
    fn check_array_pattern(&mut self, pattern: &ArrayPattern<'a>) {
        let rest = pattern.rest.as_ref().map(|r| &r.argument);
        for element in pattern.elements.iter().flatten().chain(rest) {
            match strip_default(element) {
                BindingPattern::BindingIdentifier(id) => {
                    if !id.name.as_str().starts_with('_') && self.is_unused(id) {
                        self.warn(id.span, TS_UNUSED, unused_message(id.name.as_str()));
                    }
                }
                nested => self.check_nested_pattern(nested),
            }
        }
    }

    fn check_nested_pattern(&mut self, pattern: &BindingPattern<'a>) {
        match pattern {
            BindingPattern::ObjectPattern(p) => {
                self.check_object_pattern(p, false);
            }
            BindingPattern::ArrayPattern(p) => self.check_array_pattern(p),
            BindingPattern::BindingIdentifier(_) | BindingPattern::AssignmentPattern(_) => {}
        }
    }

    /// A function/class/type declaration's name, reported when never used.
    fn check_declaration(&mut self, id: &BindingIdentifier<'a>, is_type: bool) {
        if !self.is_unused(id) {
            return;
        }
        let name = id.name.as_str();
        if is_type {
            self.warn(
                id.span,
                TS_UNUSED_TYPE,
                format!("'{name}' is declared but never used."),
            );
        } else {
            self.warn(id.span, TS_UNUSED, unused_message(name));
        }
    }

    /// tsc's unused-import grouping: a declaration whose bindings are all unused is reported
    /// as a whole when it has one binding (several: "All imports … are unused", which tsc
    /// callers drop); otherwise each unused binding is.
    fn check_imports(&mut self, declaration: &ImportDeclaration<'a>) {
        let Some(specifiers) = &declaration.specifiers else {
            return;
        };
        let type_only = declaration.import_kind.is_type();
        let mut unused: Vec<(Span, &'a str, bool)> = Vec::new();
        for specifier in specifiers {
            let (local, type_only) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    (&s.local, type_only || s.import_kind.is_type())
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => (&s.local, type_only),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (&s.local, type_only),
            };
            if !local.name.as_str().starts_with('_') && self.is_unused(local) {
                unused.push((local.span, local.name.as_str(), type_only));
            }
        }
        if unused.len() == specifiers.len() {
            if let [(_, name, _)] = unused[..] {
                self.warn(declaration.span, TS_UNUSED, unused_message(name));
            }
            return;
        }
        for (span, name, type_only) in unused {
            if type_only {
                self.warn(
                    span,
                    TS_UNUSED_TYPE,
                    format!("'{name}' is declared but never used."),
                );
            } else {
                self.warn(span, TS_UNUSED, unused_message(name));
            }
        }
    }

    /// Records the class's `private`/`#private` members for [`Self::finish_classes`].
    fn collect_private_members(&mut self, class: &Class<'a>) {
        let mut members = Vec::new();
        for element in &class.body.body {
            let (key, accessibility, method, getter, setter) = match element {
                ClassElement::MethodDefinition(m) => {
                    if m.kind == MethodDefinitionKind::Constructor {
                        continue;
                    }
                    (
                        &m.key,
                        m.accessibility,
                        Some(m.value.span),
                        m.kind == MethodDefinitionKind::Get,
                        m.kind == MethodDefinitionKind::Set,
                    )
                }
                ClassElement::PropertyDefinition(p) => {
                    if p.declare {
                        continue;
                    }
                    (&p.key, p.accessibility, None, false, false)
                }
                ClassElement::AccessorProperty(p) => (&p.key, p.accessibility, None, false, false),
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => continue,
            };
            let (name, hash) = match key {
                PropertyKey::PrivateIdentifier(id) => (id.name.as_str(), true),
                PropertyKey::StaticIdentifier(id) => (id.name.as_str(), false),
                PropertyKey::StringLiteral(s) => (s.value.as_str(), false),
                _ => continue,
            };
            if hash || accessibility == Some(TSAccessibility::Private) {
                members.push(PrivateMember {
                    name,
                    hash,
                    key: key.span(),
                    method,
                    getter,
                    setter,
                });
            }
        }
        if !members.is_empty() {
            self.classes.push(ClassInfo {
                span: class.span,
                members,
            });
        }
    }

    fn record_access(
        &mut self,
        name: &'a str,
        hash: bool,
        span: Span,
        object: Option<&Expression<'a>>,
    ) {
        self.accesses.push(Access {
            name,
            hash,
            span,
            write_only: self.write_targets.contains(&span),
            on_this: matches!(object, Some(Expression::ThisExpression(_))),
            function: self.functions.last().copied(),
        });
    }

    /// tsc's checkUnusedClassMembers: private members never read inside their class.
    fn finish_classes(&mut self) {
        let classes = std::mem::take(&mut self.classes);
        for class in &classes {
            let mut seen: HashSet<(&str, bool)> = HashSet::new();
            for member in &class.members {
                if !seen.insert((member.name, member.hash)) {
                    continue;
                }
                let group: Vec<&PrivateMember<'a>> = class
                    .members
                    .iter()
                    .filter(|m| m.name == member.name && m.hash == member.hash)
                    .collect();
                let has_getter = group.iter().any(|m| m.getter);
                let has_setter = group.iter().any(|m| m.setter);
                let read = self.accesses.iter().any(|a| {
                    a.name == member.name
                        && a.hash == member.hash
                        && class.span.start <= a.span.start
                        && a.span.end <= class.span.end
                        && !(a.write_only && !has_setter)
                        && !(a.on_this
                            && group
                                .iter()
                                .any(|m| m.method.is_some() && m.method == a.function))
                });
                if read {
                    continue;
                }
                let shown = if member.hash {
                    format!("#{}", member.name)
                } else {
                    member.name.to_string()
                };
                for m in group {
                    if m.setter && has_getter {
                        continue;
                    }
                    self.warn(m.key, TS_UNUSED, unused_message(&shown));
                }
            }
        }
    }

    /// Runs `f` as a fresh function body: reachable, with its own jump targets. `deferred`
    /// bodies (function expressions, arrows, class expressions) are checked by tsc after
    /// their enclosing statement, so an unreachable report around them doesn't cover them.
    fn in_function(&mut self, span: Option<Span>, deferred: bool, f: impl FnOnce(&mut Self)) {
        let flow = std::mem::replace(&mut self.flow, REACHABLE);
        let targets = std::mem::take(&mut self.targets);
        let pending = std::mem::take(&mut self.pending_labels);
        let for_in_of = std::mem::replace(&mut self.in_for_in_of_left, false);
        let suppress = self.suppress;
        if deferred {
            self.suppress = false;
        }
        if let Some(span) = span {
            self.functions.push(span);
        }
        f(self);
        if span.is_some() {
            self.functions.pop();
        }
        self.flow = flow;
        self.targets = targets;
        self.pending_labels = pending;
        self.in_for_in_of_left = for_in_of;
        self.suppress = suppress;
    }

    /// Whether an expression statement calls a function declared to return `never`.
    fn calls_never(&self, expression: &Expression<'a>) -> bool {
        let Expression::CallExpression(call) = expression else {
            return false;
        };
        if call.optional {
            return false;
        }
        let Expression::Identifier(callee) = &call.callee else {
            return false;
        };
        let Some(reference) = callee.reference_id.get() else {
            return false;
        };
        let scoping = self.semantic.scoping();
        let Some(symbol) = scoping.get_reference(reference).symbol_id() else {
            return false;
        };
        scoping.symbol_declarations(symbol).any(|node| {
            matches!(
                self.semantic.nodes().kind(node),
                AstKind::Function(f) if f.return_type.as_ref().is_some_and(|t| matches!(t.type_annotation, TSType::TSNeverKeyword(_)))
            )
        })
    }

    fn pop_target(&mut self) -> Target<'a> {
        self.targets
            .pop()
            .unwrap_or_else(|| Target::new(TargetKind::Label, Vec::new()))
    }
}

fn unused_message(name: &str) -> String {
    format!("'{name}' is declared but its value is never read.")
}

fn strip_default<'b, 'a>(pattern: &'b BindingPattern<'a>) -> &'b BindingPattern<'a> {
    match pattern {
        BindingPattern::AssignmentPattern(p) => strip_default(&p.left),
        other => other,
    }
}

/// tsc's isPotentiallyExecutableNode (minus const enums, which are never reported).
fn is_executable(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::VariableDeclaration(d) => {
            d.kind != VariableDeclarationKind::Var
                || d.declarations.iter().any(|d| d.init.is_some())
        }
        Statement::TSEnumDeclaration(e) => !e.r#const,
        Statement::ExpressionStatement(_)
        | Statement::IfStatement(_)
        | Statement::DoWhileStatement(_)
        | Statement::WhileStatement(_)
        | Statement::ForStatement(_)
        | Statement::ForInStatement(_)
        | Statement::ForOfStatement(_)
        | Statement::ContinueStatement(_)
        | Statement::BreakStatement(_)
        | Statement::ReturnStatement(_)
        | Statement::WithStatement(_)
        | Statement::SwitchStatement(_)
        | Statement::LabeledStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::TryStatement(_)
        | Statement::DebuggerStatement(_)
        | Statement::ClassDeclaration(_) => true,
        _ => false,
    }
}

fn is_loop_or_label(statement: &Statement<'_>) -> bool {
    matches!(
        statement,
        Statement::DoWhileStatement(_)
            | Statement::WhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::LabeledStatement(_)
    )
}

/// Whether a condition can be (true, false), folding literal `true`/`false` through `&&`/`||`
/// the way tsc's binder does (a parenthesized or negated literal is not folded).
fn condition(expression: &Expression<'_>) -> (bool, bool) {
    match expression {
        Expression::BooleanLiteral(b) => (b.value, !b.value),
        Expression::LogicalExpression(e) => {
            let (lt, lf) = condition(&e.left);
            let (rt, rf) = condition(&e.right);
            match e.operator {
                LogicalOperator::And => (lt && rt, lf || (lt && rf)),
                LogicalOperator::Or => (lt || (lf && rt), lf && rf),
                LogicalOperator::Coalesce => (true, true),
            }
        }
        Expression::ParenthesizedExpression(p) if is_logical(&p.expression) => {
            condition(&p.expression)
        }
        Expression::UnaryExpression(u)
            if u.operator == UnaryOperator::LogicalNot && is_logical(&u.argument) =>
        {
            let (t, f) = condition(&u.argument);
            (f, t)
        }
        _ => (true, true),
    }
}

/// tsc's isLogicalExpression: `&&`/`||`/`??` under parentheses and `!`.
fn is_logical(expression: &Expression<'_>) -> bool {
    match expression {
        Expression::ParenthesizedExpression(p) => is_logical(&p.expression),
        Expression::UnaryExpression(u) if u.operator == UnaryOperator::LogicalNot => {
            is_logical(&u.argument)
        }
        Expression::LogicalExpression(_) => true,
        _ => false,
    }
}

impl<'a> Visit<'a> for Analyzer<'_, 'a> {
    fn visit_statements(&mut self, statements: &oxc_allocator::Vec<'a, Statement<'a>>) {
        let mut i = 0;
        while i < statements.len() {
            if !self.suppress && self.flow & LIVE == 0 && is_executable(&statements[i]) {
                // One report for each run of unreachable executable statements.
                let mut last = i;
                while last + 1 < statements.len() && is_executable(&statements[last + 1]) {
                    last += 1;
                }
                self.unreachable(Span::new(
                    statements[i].span().start,
                    statements[last].span().end,
                ));
                self.suppress = true;
                for statement in &statements[i..=last] {
                    self.visit_statement(statement);
                }
                self.suppress = false;
                i = last + 1;
            } else {
                self.visit_statement(&statements[i]);
                i += 1;
            }
        }
    }

    fn visit_statement(&mut self, statement: &Statement<'a>) {
        if !self.suppress && self.flow & LIVE == 0 && is_executable(statement) {
            self.unreachable(statement.span());
            self.suppress = true;
            walk::walk_statement(self, statement);
            self.suppress = false;
        } else {
            walk::walk_statement(self, statement);
        }
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        walk::walk_expression_statement(self, it);
        if self.flow & LIVE != 0 && self.calls_never(&it.expression) {
            self.flow &= !LIVE;
        }
    }

    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        walk::walk_return_statement(self, it);
        self.flow = 0;
    }

    fn visit_throw_statement(&mut self, it: &ThrowStatement<'a>) {
        walk::walk_throw_statement(self, it);
        self.flow = 0;
    }

    fn visit_break_statement(&mut self, it: &BreakStatement<'a>) {
        let flow = self.flow;
        let target = match &it.label {
            Some(label) => {
                let name = label.name.as_str();
                self.targets
                    .iter_mut()
                    .rev()
                    .find(|t| t.kind == TargetKind::Label && t.labels.contains(&name))
            }
            None => self
                .targets
                .iter_mut()
                .rev()
                .find(|t| t.kind != TargetKind::Label),
        };
        if let Some(target) = target {
            target.breaks |= flow;
            target.referenced |= flow & BOUND != 0;
        }
        self.flow = 0;
    }

    fn visit_continue_statement(&mut self, it: &ContinueStatement<'a>) {
        let flow = self.flow;
        match &it.label {
            Some(label) => {
                let name = label.name.as_str();
                if let Some(t) = self
                    .targets
                    .iter_mut()
                    .rev()
                    .find(|t| t.kind == TargetKind::Label && t.labels.contains(&name))
                {
                    t.referenced |= flow & BOUND != 0;
                }
                if let Some(t) = self
                    .targets
                    .iter_mut()
                    .rev()
                    .find(|t| t.kind == TargetKind::Loop && t.labels.contains(&name))
                {
                    t.continues |= flow;
                }
            }
            None => {
                if let Some(t) = self
                    .targets
                    .iter_mut()
                    .rev()
                    .find(|t| t.kind == TargetKind::Loop)
                {
                    t.continues |= flow;
                }
            }
        }
        self.flow = 0;
    }

    fn visit_labeled_statement(&mut self, it: &LabeledStatement<'a>) {
        // tsc's binder skips labels in unreachable code, so they're never reported unused.
        let bound = self.flow & BOUND != 0;
        let name = it.label.name.as_str();
        self.targets
            .push(Target::new(TargetKind::Label, vec![name]));
        if is_loop_or_label(&it.body) {
            self.pending_labels.push(name);
        } else {
            self.pending_labels.clear();
        }
        self.visit_statement(&it.body);
        let target = self.pop_target();
        self.flow |= target.breaks;
        if bound && !target.referenced {
            self.warn(it.label.span, TS_UNUSED_LABEL, "Unused label.".to_string());
        }
    }

    fn visit_if_statement(&mut self, it: &IfStatement<'a>) {
        self.visit_expression(&it.test);
        let entry = self.flow;
        let (can_true, can_false) = condition(&it.test);
        self.flow = if can_true { entry } else { 0 };
        self.visit_statement(&it.consequent);
        let then_end = self.flow;
        self.flow = if can_false { entry } else { 0 };
        if let Some(alternate) = &it.alternate {
            self.visit_statement(alternate);
        }
        self.flow |= then_end;
    }

    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.visit_expression(&it.test);
        let entry = self.flow;
        let (can_true, can_false) = condition(&it.test);
        self.targets.push(Target::new(TargetKind::Loop, labels));
        self.flow = if can_true { entry } else { 0 };
        self.visit_statement(&it.body);
        let target = self.pop_target();
        self.flow = (if can_false { entry } else { 0 }) | target.breaks;
    }

    fn visit_do_while_statement(&mut self, it: &DoWhileStatement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.targets.push(Target::new(TargetKind::Loop, labels));
        self.visit_statement(&it.body);
        let target = self.pop_target();
        let head = self.flow | target.continues;
        self.flow = head;
        self.visit_expression(&it.test);
        let (_, can_false) = condition(&it.test);
        self.flow = (if can_false { head } else { 0 }) | target.breaks;
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        if let Some(init) = &it.init {
            self.visit_for_statement_init(init);
        }
        if let Some(test) = &it.test {
            self.visit_expression(test);
        }
        let entry = self.flow;
        let (can_true, can_false) = it.test.as_ref().map_or((true, false), condition);
        self.targets.push(Target::new(TargetKind::Loop, labels));
        self.flow = if can_true { entry } else { 0 };
        self.visit_statement(&it.body);
        let target = self.pop_target();
        self.flow |= target.continues;
        if let Some(update) = &it.update {
            self.visit_expression(update);
        }
        self.flow = (if can_false { entry } else { 0 }) | target.breaks;
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.visit_for_left(&it.left);
        self.visit_expression(&it.right);
        self.loop_body_always_exits(labels, &it.body);
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        let labels = std::mem::take(&mut self.pending_labels);
        self.visit_for_left(&it.left);
        self.visit_expression(&it.right);
        self.loop_body_always_exits(labels, &it.body);
    }

    fn visit_switch_statement(&mut self, it: &SwitchStatement<'a>) {
        self.pending_labels.clear();
        self.visit_expression(&it.discriminant);
        let entry = self.flow;
        self.targets
            .push(Target::new(TargetKind::Switch, Vec::new()));
        let mut fallthrough = 0;
        let mut has_default = false;
        for case in &it.cases {
            self.flow = entry;
            match &case.test {
                Some(test) => self.visit_expression(test),
                None => has_default = true,
            }
            self.flow = entry | fallthrough;
            self.visit_statements(&case.consequent);
            fallthrough = self.flow;
        }
        let target = self.pop_target();
        self.flow = fallthrough | target.breaks | if has_default { 0 } else { entry };
    }

    fn visit_try_statement(&mut self, it: &TryStatement<'a>) {
        let entry = self.flow;
        self.visit_block_statement(&it.block);
        let mut normal = self.flow;
        if let Some(handler) = &it.handler {
            self.flow = entry;
            self.visit_catch_clause(handler);
            normal |= self.flow;
        }
        if let Some(finalizer) = &it.finalizer {
            self.flow = entry;
            self.visit_block_statement(finalizer);
            normal &= self.flow;
        }
        self.flow = normal;
    }

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        self.declare_error(it.declare, it.span);
        if !it.declare {
            self.check_variable_list(it);
        }
        let for_in_of = std::mem::replace(&mut self.in_for_in_of_left, false);
        walk::walk_variable_declaration(self, it);
        self.in_for_in_of_left = for_in_of;
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if matches!(
            it.r#type,
            FunctionType::FunctionDeclaration | FunctionType::TSDeclareFunction
        ) && let Some(id) = &it.id
        {
            self.declare_error(it.declare, it.span);
            if !it.declare {
                self.check_declaration(id, false);
            }
        }
        let deferred =
            it.r#type == FunctionType::FunctionExpression && !self.class_methods.contains(&it.span);
        self.in_function(Some(it.span), deferred, |this| {
            walk::walk_function(this, it, flags)
        });
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.in_function(Some(it.span), true, |this| {
            walk::walk_arrow_function_expression(this, it)
        });
    }

    fn visit_static_block(&mut self, it: &StaticBlock<'a>) {
        self.in_function(None, false, |this| walk::walk_static_block(this, it));
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        if it.r#type == ClassType::ClassDeclaration
            && let Some(id) = &it.id
        {
            self.declare_error(it.declare, it.span);
            if !it.declare {
                self.check_declaration(id, true);
            }
        }
        self.collect_private_members(it);
        let suppress = self.suppress;
        if it.r#type == ClassType::ClassExpression {
            self.suppress = false;
        }
        walk::walk_class(self, it);
        self.suppress = suppress;
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        self.class_methods.insert(it.value.span);
        walk::walk_method_definition(self, it);
    }

    fn visit_ts_interface_declaration(&mut self, it: &TSInterfaceDeclaration<'a>) {
        self.declare_error(it.declare, it.span);
        if !it.declare {
            self.check_declaration(&it.id, true);
        }
        walk::walk_ts_interface_declaration(self, it);
    }

    fn visit_ts_type_alias_declaration(&mut self, it: &TSTypeAliasDeclaration<'a>) {
        self.declare_error(it.declare, it.span);
        if !it.declare {
            self.check_declaration(&it.id, true);
        }
        walk::walk_ts_type_alias_declaration(self, it);
    }

    fn visit_ts_enum_declaration(&mut self, it: &TSEnumDeclaration<'a>) {
        self.declare_error(it.declare, it.span);
        if !it.declare {
            self.check_declaration(&it.id, true);
        }
        walk::walk_ts_enum_declaration(self, it);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        if !self.exported.contains(&it.span.start) {
            self.keyword_error(it.span.start, "import".len() as u32, IMPORT_NOT_TOP_LEVEL);
        }
        if self.is_unused(&it.id) && !it.id.name.as_str().starts_with('_') {
            let name = it.id.name.as_str();
            if it.import_kind.is_type() {
                self.warn(
                    it.id.span,
                    TS_UNUSED_TYPE,
                    format!("'{name}' is declared but never used."),
                );
            } else {
                self.warn(it.id.span, TS_UNUSED, unused_message(name));
            }
        }
        walk::walk_ts_import_equals_declaration(self, it);
    }

    fn visit_module_declaration(&mut self, it: &ModuleDeclaration<'a>) {
        let start = it.span().start;
        let export = "export".len() as u32;
        match it {
            ModuleDeclaration::ImportDeclaration(d) => {
                self.keyword_error(start, "import".len() as u32, IMPORT_NOT_TOP_LEVEL);
                if !self.functions.is_empty() {
                    self.check_imports(d);
                }
            }
            ModuleDeclaration::ExportDeclaration(d) => {
                let message = if matches!(d.declaration, Declaration::TSImportEqualsDeclaration(_))
                {
                    IMPORT_NOT_TOP_LEVEL
                } else {
                    MODIFIERS_HERE
                };
                self.keyword_error(start, export, message);
                self.exported.insert(d.declaration.span().start);
            }
            ModuleDeclaration::ExportAllDeclaration(_)
            | ModuleDeclaration::ExportNamedDeclaration(_)
            | ModuleDeclaration::ExportFromDeclaration(_) => {
                self.keyword_error(start, export, EXPORT_NOT_TOP_LEVEL);
            }
            ModuleDeclaration::ExportDefaultDeclaration(_) => self.keyword_error(
                start,
                export,
                "A default export must be at the top level of a file or module declaration.",
            ),
            ModuleDeclaration::TSExportAssignment(_) => self.keyword_error(
                start,
                export,
                "An export assignment must be at the top level of a file or module declaration.",
            ),
            ModuleDeclaration::TSNamespaceExportDeclaration(_) => {
                self.keyword_error(
                    start,
                    export,
                    "Global module exports may only appear at top level.",
                );
            }
        }
        walk::walk_module_declaration(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator == AssignmentOperator::Assign
            && let Some(member) = it.left.as_member_expression()
        {
            self.write_targets.insert(member.span());
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.record_access(it.property.name.as_str(), false, it.span, Some(&it.object));
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::StringLiteral(key) = &it.expression {
            self.record_access(key.value.as_str(), false, it.span, Some(&it.object));
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_private_field_expression(&mut self, it: &PrivateFieldExpression<'a>) {
        self.record_access(it.field.name.as_str(), true, it.span, Some(&it.object));
        walk::walk_private_field_expression(self, it);
    }

    fn visit_private_in_expression(&mut self, it: &PrivateInExpression<'a>) {
        self.record_access(it.left.name.as_str(), true, it.span, None);
        walk::walk_private_in_expression(self, it);
    }
}

impl<'a> Analyzer<'_, 'a> {
    /// The left side of a for-in/for-of: `_`-prefixed declared names are exempt, member targets are writes.
    fn visit_for_left(&mut self, left: &ForStatementLeft<'a>) {
        if let Some(member) = left.as_member_expression() {
            self.write_targets.insert(member.span());
        }
        self.in_for_in_of_left = matches!(left, ForStatementLeft::VariableDeclaration(_));
        self.visit_for_statement_left(left);
        self.in_for_in_of_left = false;
    }

    /// A for-in/for-of body: the loop may run zero times, so the code after it is reachable from the entry.
    fn loop_body_always_exits(&mut self, labels: Vec<&'a str>, body: &Statement<'a>) {
        let entry = self.flow;
        self.targets.push(Target::new(TargetKind::Loop, labels));
        self.visit_statement(body);
        let target = self.pop_target();
        self.flow = entry | target.breaks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(blocks: &[&str]) -> Vec<Vec<LintProblem>> {
        lint_tool_code(&blocks.iter().map(|b| (*b).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn empty_blocks_have_no_problems() {
        assert_eq!(lint(&[]), Vec::<Vec<LintProblem>>::new());
        assert_eq!(lint(&["", "  \n\t"]), vec![vec![], vec![]]);
    }

    #[test]
    fn used_across_blocks() {
        assert_eq!(lint(&["const x = 1;", "return x"]), vec![vec![], vec![]]);
    }

    #[test]
    fn unused_local() {
        let out = lint(&["const a = 1;"]);
        assert_eq!(
            out,
            vec![vec![LintProblem {
                from: 6,
                to: 7,
                line: 1,
                col: 7,
                message: "'a' is declared but its value is never read.".into(),
                severity: Severity::Warning,
            }]]
        );
    }

    #[test]
    fn redeclared_block_scoped() {
        let out = lint(&["let a = 1;\nlet a = 2;\nreturn a;"]);
        let errors: Vec<_> = out[0]
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|p| p.message == "Cannot redeclare block-scoped variable 'a'.")
        );
        assert_eq!(
            (errors[0].from, errors[0].to, errors[1].line, errors[1].col),
            (4, 5, 2, 5)
        );
    }

    #[test]
    fn var_after_let_in_nested_block_is_not_a_redeclaration() {
        let out = lint(&["{ let a; var a; }"]);
        assert!(
            out[0].iter().all(|p| p.severity == Severity::Warning),
            "{out:?}"
        );
        let out = lint(&["let input = 1; return input;"]);
        assert!(out[0].is_empty(), "{out:?}");
    }

    #[test]
    fn unreachable_after_return() {
        let out = lint(&["return 1;\nconst y = 2;"]);
        let messages: Vec<_> = out[0]
            .iter()
            .map(|p| (p.from, p.to, p.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            vec![
                (10, 22, "Unreachable code detected."),
                (16, 17, "'y' is declared but its value is never read."),
            ]
        );
    }

    #[test]
    fn syntax_error() {
        let out = lint(&["const x = (;"]);
        assert_eq!(out[0].len(), 1);
        assert_eq!(out[0][0].severity, Severity::Error);
        assert!(out[0][0].to > out[0][0].from && out[0][0].to <= 12);
    }

    #[test]
    fn unclosed_brace_maps_into_last_block() {
        let out = lint(&["if (x) {"]);
        assert!(!out[0].is_empty());
        for p in &out[0] {
            assert_eq!(p.severity, Severity::Error);
            assert!(p.from < p.to && p.to <= 8, "{p:?}");
        }
    }

    #[test]
    fn offsets_are_relative_to_their_block() {
        // The whole second block is unreachable: one run, from its first byte to its last.
        let out = lint(&["return input;", "let 好 = 1;\nconst b = 2;"]);
        assert!(out[0].is_empty());
        let run = &out[1][0];
        assert_eq!(
            (run.from, run.to, run.line, run.col, run.message.as_str()),
            (0, 25, 1, 1, "Unreachable code detected.")
        );

        let out = lint(&["const v = input;", "let 好 = v;\nconst b = 2;"]);
        let b = out[1]
            .iter()
            .find(|p| p.message.starts_with("'b'"))
            .expect("b unused");
        assert_eq!((b.from, b.to, b.line, b.col), (19, 20, 2, 7));
        let hao = out[1]
            .iter()
            .find(|p| p.message.starts_with("'好'"))
            .expect("好 unused");
        assert_eq!((hao.from, hao.to, hao.line, hao.col), (4, 7, 1, 5));
    }

    #[test]
    fn unused_label() {
        let out = lint(&["foo: for(;;){break;}"]);
        assert_eq!(out[0].len(), 1);
        assert_eq!(
            (out[0][0].from, out[0][0].to, out[0][0].message.as_str()),
            (0, 3, "Unused label.")
        );
        assert_eq!(out[0][0].severity, Severity::Warning);
        assert!(lint(&["foo: for(;;){break foo;}"])[0].is_empty());
    }

    #[test]
    fn destructuring_and_lists() {
        let msg = |src: &str| -> Vec<(usize, usize, String)> {
            lint(&[src])[0]
                .iter()
                .map(|p| (p.from, p.to, p.message.clone()))
                .collect()
        };
        assert_eq!(
            msg("const {a, b} = input; return 1;")[0],
            (6, 12, "All destructured elements are unused.".into())
        );
        assert_eq!(
            msg("const x = 1, y = 2;"),
            vec![(0, 19, "All variables are unused.".into())]
        );
        assert_eq!(
            msg("const {a} = input;"),
            vec![(6, 9, "'a' is declared but its value is never read.".into())]
        );
        assert!(msg("const {a, ...r} = input; return r;").is_empty());
        assert_eq!(
            msg("class C{}"),
            vec![(6, 7, "'C' is declared but never used.".into())]
        );
        assert_eq!(
            msg("class C { #p = 1; }\nreturn C;"),
            vec![(
                10,
                12,
                "'#p' is declared but its value is never read.".into()
            )]
        );
    }
}

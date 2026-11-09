//! The graph page against the daemon named by the environment. Arguments: the space to show
//! (default: the remembered one), then optionally an object id to focus (pinned and zoomed
//! to, like `/app/graph?focus=<id>`).
//!
//! `cargo run -p roostr-graph --example graph -- <space-id> [object-id] --snap out.png`
//! (`ROOSTR_UI_STATE=<scratch file>` keeps your own tabs and space untouched).

use roostr_core::State;
use roostr_core::nav::{self, Route};

fn register(s: &mut State) {
    roostr_graph::register(s);
    let mut args = rune::extra_args().iter().cloned();
    if let Some(space) = args.next().filter(|a| !a.is_empty()) {
        s.nav.use_space(&space, &mut s.ui);
    }
    nav::navigate(Route::Graph { focus: args.next() }, s);
    // A headless `--snap` draws at once: let the store load and the graph build first.
    roostr_host::wait_loaded(s, std::time::Duration::from_secs(20));
    roostr_host::settle(s, std::time::Duration::from_secs(20));
}

fn main() {
    roostr_host::run_example("Graph", register, roostr_graph::view);
}

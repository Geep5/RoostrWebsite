//! The graph page against the test daemon. Optional argument: an object id
//! to focus (pinned and zoomed to, like `/app/graph?focus=<id>`).

use gpui_kit::AppContext as _;
use roostr_graph::GraphView;

fn main() {
    roostr_core::dev::run_example("Graph", |args, window, cx| {
        roostr_graph::init(cx);
        let focus = args.first().cloned();
        cx.new(|cx| GraphView::new(focus, window, cx)).into()
    });
}

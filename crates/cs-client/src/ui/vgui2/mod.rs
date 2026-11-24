//! Declarative Valve VGUI2 interpreter, 12-control library, MOTD HTML subset renderer,
//! and `.res` layout runtime with server/mod override hot-reloading.

pub mod controls;
pub mod html;
pub mod runtime;

pub use controls::{
    ListColumn, ListSortDir, VguiComboState, VguiControlState, VguiListPanelState,
    VguiPropertySheetState, VguiRichTextState, VguiSliderState,
};
pub use html::{HtmlBlock, HtmlDoc, HtmlSpan, parse_html_doc, paint_html_doc};
pub use runtime::{
    VguiEvent, VguiLayoutNode, VguiRuntime, install_marlett_into_egui, resolve_vgui_text_color,
};

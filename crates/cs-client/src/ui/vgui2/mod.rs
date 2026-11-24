//! Declarative Valve VGUI2 interpreter, 12-control library, MOTD HTML subset renderer,
//! and `.res` layout runtime with server/mod override hot-reloading.

pub mod controls;
pub mod font_atlas;
pub mod html;
pub mod runtime;

pub use controls::{
    ListColumn, ListSortDir, VguiComboState, VguiControlState, VguiListPanelState,
    VguiPropertySheetState, VguiRichTextState, VguiSliderState,
};
pub use font_atlas::{VguiFontAtlas, VguiTextDrawOpts};
pub use html::{HtmlBlock, HtmlDoc, HtmlSpan, paint_html_doc, paint_html_doc_vgui, parse_html_doc};
pub use runtime::{
    VguiEvent, VguiLayoutNode, VguiRuntime, VguiSchemeKind, VguiUiSound, install_marlett_into_egui,
    install_scheme_fonts_into_egui, modal_backdrop, resolve_vgui_text_color,
};

//! Declarative Valve VGUI2 interpreter, 12-control library, MOTD HTML subset renderer,
//! and `.res` layout runtime with server/mod override hot-reloading.

pub mod controls;
pub mod font_atlas;
pub mod html;
pub mod runtime;

pub use controls::{
    ListColumn, ListSortDir, TrackerCtx, VguiComboState, VguiControlState, VguiListPanelState,
    VguiPropertySheetState, VguiRichTextState, VguiSliderState, draw_tracker_button_rect,
    draw_tracker_check_button_rect, draw_tracker_close_button, draw_tracker_combo_box_rect,
    draw_tracker_list_panel_rect, draw_tracker_radio_button_rect, draw_tracker_slider_rect,
    draw_tracker_text_entry_rect, paint_tracker_bevel_rect, paint_tracker_label_rect,
};
pub use font_atlas::{VguiFontAtlas, VguiTextDrawOpts};
pub use html::{HtmlBlock, HtmlDoc, HtmlSpan, paint_html_doc, paint_html_doc_vgui, parse_html_doc};
pub use runtime::{
    TrackerButtonCfg, VguiEvent, VguiLayoutNode, VguiRuntime, VguiSchemeKind, VguiUiSound,
    client_ui_viewport, install_marlett_into_egui, install_scheme_fonts_into_egui, modal_backdrop,
    paint_vgui_border, resolve_vgui_text_color,
};

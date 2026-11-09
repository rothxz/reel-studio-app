//! The editor's panels. Each renders from the `Editor` state; edits go back
//! through `Editor` methods so every change is one undoable step.

pub mod adjust_panel;
pub mod agent;
pub mod audio_panel;
pub mod browser;
pub mod caption_edit;
pub mod captions;
pub mod clip_tools;
pub mod color_row;
pub mod crop;
pub mod dialogs;
pub mod export;
pub mod fx;
pub mod gizmo;
pub mod header;
pub mod home;
pub mod icons;
pub mod inspector;
pub mod layers;
pub mod library;
pub mod player;
pub mod settings;
pub mod speed_curve;
pub mod storage;
pub mod text_input;
pub mod text_panel;
pub mod timeline;
pub mod timeline_edit;
pub mod toolbar;
pub mod transitions;
pub mod update_dialog;
pub mod visual;
pub mod widgets;
pub mod window_chrome;

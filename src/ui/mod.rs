//! The editor's panels. Each renders from the `Editor` state; edits go back
//! through `Editor` methods so every change is one undoable step.

pub mod audio_panel;
pub mod browser;
pub mod dialogs;
pub mod fx;
pub mod gizmo;
pub mod header;
pub mod home;
pub mod icons;
pub mod inspector;
pub mod library;
pub mod player;
pub mod text_input;
pub mod text_panel;
pub mod timeline;
pub mod timeline_edit;
pub mod toolbar;
pub mod visual;
pub mod widgets;

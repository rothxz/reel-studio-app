//! The editor's panels. Each renders from the `Editor` state; edits go back
//! through `Editor` methods so every change is one undoable step.

pub mod browser;
pub mod dialogs;
pub mod fx;
pub mod header;
pub mod home;
pub mod icons;
pub mod inspector;
pub mod player;
pub mod timeline;
pub mod timeline_edit;
pub mod toolbar;
pub mod visual;
pub mod widgets;

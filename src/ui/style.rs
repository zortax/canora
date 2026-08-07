//! The sheet the whole window is drawn by.
//!
//! The rules live in `assets/app.css`. The first block there tightens the component library:
//! `--zui-space-base` is the unit every gap and padding in it is a multiple of, and
//! `--zui-radius-base` is the unit every corner is a multiple of.
//!
//! The sheet is read as a file rather than through `css!`, so it can be edited beside the themes.
//! The style engine reports anything it cannot read when the window opens.

/// The application sheet.
pub const SHEET: &str = include_str!("../../assets/app.css");

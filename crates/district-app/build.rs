//! Compiles `data/` (the `.ui` templates, the stylesheet, the ringtone and the
//! icons) into the gresource the binary carries, with the
//! `glib-compile-resources` that ships beside the GTK development files.

fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/resources.gresource.xml",
        "district-app.gresource",
    );
}

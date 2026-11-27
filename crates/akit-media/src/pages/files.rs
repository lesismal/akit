// Written by tools/media-pages.py from fit's handler/mediapages/web at the commit below; do
// not edit. The files are fit's, under fit's licence (../../pages/LICENSE), with fit's names
// renamed as akit names them (../../pages/CHANGES, decision 037).

/// The fit commit the files were copied from: the compatibility gate's pin.
pub const FIT_COMMIT: &str = "7daee573b102d87837cc10f033e4cc5894769210";

/// Each file: its name under the pages' prefix, and its content.
#[rustfmt::skip]
pub(crate) const FILES: &[(&str, &[u8])] = &[
    ("app.css", include_bytes!("../../pages/web/app.css")),
    ("app.js", include_bytes!("../../pages/web/app.js")),
    ("index.html", include_bytes!("../../pages/web/index.html")),
];

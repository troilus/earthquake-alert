use html_minifier::HTMLMinifier;
use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=web/index.html");

    let source = fs::read("web/index.html")?;
    let mut minifier = HTMLMinifier::new();
    minifier.digest(source).map_err(io::Error::other)?;

    let output = PathBuf::from(
        env::var_os("OUT_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Cargo did not set OUT_DIR"))?,
    )
    .join("index.min.html");
    fs::write(output, minifier.get_html())
}

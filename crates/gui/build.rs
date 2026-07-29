//! Compiles the UI icon PNGs into a gresource so tab icons resolve
//! from the icon theme by name (travelmode-ui-<name>-<variant>).

use std::process::Command;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let target = format!("{out_dir}/ui-icons.gresource");
    let status = Command::new("glib-compile-resources")
        .arg(format!("--sourcedir={}", concat!(env!("CARGO_MANIFEST_DIR"), "/../..")))
        .arg(format!("--target={target}"))
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/ui-icons.gresource.xml"))
        .status()
        .expect("failed to run glib-compile-resources (install glib2 development tools)");
    assert!(status.success(), "glib-compile-resources failed");

    println!("cargo:rerun-if-changed=ui-icons.gresource.xml");
    println!("cargo:rerun-if-changed=../../data/icons/ui/rendered");
}

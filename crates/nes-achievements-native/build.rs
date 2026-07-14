use std::{fs, path::Path};

fn collect_c_files(root: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("could not read vendored rcheevos source") {
        let path = entry.expect("invalid rcheevos directory entry").path();
        if path.is_dir() {
            collect_c_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "c") {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !matches!(
                name,
                "rc_client_external.c"
                    | "rc_client_raintegration.c"
                    | "rc_libretro.c"
                    | "cdreader.c"
                    | "hash_disc.c"
                    | "hash_encrypted.c"
                    | "hash_zip.c"
                    | "aes.c"
            ) {
                files.push(path);
            }
        }
    }
}

fn main() {
    let vendor = Path::new("native/rcheevos");
    let mut files = Vec::new();
    collect_c_files(&vendor.join("src"), &mut files);
    files.sort();

    println!("cargo:rerun-if-changed=native/nes_ra.c");
    println!("cargo:rerun-if-changed=native/nes_ra.h");
    println!("cargo:rerun-if-changed=native/rcheevos/include");
    println!("cargo:rerun-if-changed=native/rcheevos/src");

    cc::Build::new()
        .file("native/nes_ra.c")
        .files(files)
        .include("native")
        .include(vendor.join("include"))
        .define("RC_DISABLE_LUA", None)
        .define("RC_CLIENT_SUPPORTS_HASH", None)
        .define("RC_STATIC", None)
        .define("RC_HASH_NO_DISC", None)
        .define("RC_HASH_NO_ENCRYPTED", None)
        .define("RC_HASH_NO_ZIP", None)
        .warnings(false)
        .compile("nes_rcheevos_native");
}

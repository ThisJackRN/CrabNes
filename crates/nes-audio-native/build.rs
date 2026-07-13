fn main() {
    println!("cargo:rerun-if-changed=native/nes_audio.c");
    println!("cargo:rerun-if-changed=native/vendor/miniaudio.c");
    println!("cargo:rerun-if-changed=native/vendor/miniaudio.h");

    let mut build = cc::Build::new();
    build
        .file("native/nes_audio.c")
        .include("native/vendor")
        .define("MA_NO_DECODING", None)
        .define("MA_NO_ENCODING", None)
        .define("MA_NO_RESOURCE_MANAGER", None)
        .define("MA_NO_NODE_GRAPH", None)
        .warnings(false);

    if cfg!(target_os = "windows") {
        build
            .define("MA_ENABLE_ONLY_SPECIFIC_BACKENDS", None)
            .define("MA_ENABLE_WASAPI", None);
    }

    build.compile("nes_audio_native");
}

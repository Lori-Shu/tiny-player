fn main() {
    // let mut dlls = vec![];
    // let folder_path = Path::new("./resources/dlls");
    // if let Ok(dir) = folder_path.read_dir() {
    //     for p in dir.flatten() {
    //         if p.file_name().to_str().unwrap().ends_with(".dll") {
    //             dlls.push(p.path());
    //         }
    //     }
    // }
    // for dll in dlls {
    //     let target_debug_dest =
    //         Path::new("target/debug").join(dll.file_name().unwrap().to_str().unwrap());
    //     fs::copy(&dll, target_debug_dest).unwrap();
    // }
    // if fs::read_dir("target/debug/model").is_err() {
    //     fs::create_dir("target/debug/model").unwrap();
    // }
    // fs::copy(
    //     "resources/model/base_q8_0.gguf",
    //     "target/debug/model/base_q8_0.gguf",
    // )
    // .unwrap();
    // fs::copy(
    //     "resources/model/config.json",
    //     "target/debug/model/config.json",
    // )
    // .unwrap();
    // fs::copy(
    //     "resources/model/tokenizer.json",
    //     "target/debug/model/tokenizer.json",
    // )
    // .unwrap();
    if std::env::var("CARGO_CFG_TARGET_FAMILY").unwrap() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("resources/play.ico");
        res.compile().unwrap();
    }
}

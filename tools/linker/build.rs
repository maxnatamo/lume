#![allow(clippy::disallowed_macros)]
fn main() {
    let cwd = std::env::current_dir().expect("current directory");
    let testfiles = cwd.join("testfiles");

    for file in testfiles.read_dir().unwrap() {
        let source_file = file.unwrap();
        let source_file_path = source_file.path();

        let is_c_file = source_file_path
            .extension()
            .is_some_and(|ext| ext.to_str().unwrap() == "c");

        if !source_file.file_type().unwrap().is_file() || !is_c_file {
            continue;
        }

        println!("cargo::rerun-if-changed={}", source_file_path.display());

        let mut object_file_path = source_file_path.clone();

        #[cfg(target_os = "macos")]
        object_file_path.set_extension("macho.o");

        #[cfg(target_os = "linux")]
        object_file_path.set_extension("elf.o");

        #[cfg(target_os = "windows")]
        object_file_path.set_extension("pe.o");

        for object_path in cc::Build::new().file(source_file_path).compile_intermediates() {
            std::fs::copy(object_path, &object_file_path).unwrap();
        }
    }
}

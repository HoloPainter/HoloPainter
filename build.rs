use std::fs;
use std::path::Path;

fn main() {
    generate_embedded_resources();

    println!("cargo:rerun-if-changed=assets/app_icon/app_icon.ico");
    println!("cargo:rerun-if-changed=assets/app_icon/windows.rc");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("assets/app_icon/windows.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed the Windows application icon");
    }

    // Tell Cargo to rerun this script if any file in src/shaders changes
    println!("cargo:rerun-if-changed=src/shaders");

    let shader_dir = Path::new("src/shaders");
    if !shader_dir.exists() {
        return;
    }

    let entries = fs::read_dir(shader_dir).expect("Failed to read shader directory");
    for entry in entries {
        let entry = entry.expect("Failed to read directory entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("wgsl") {
            let shader_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let source = fs::read_to_string(&path).expect("Failed to read shader file");

            if let Err(e) = validate_wgsl(&source) {
                panic!("\n\n[WGSL Error] in {}:\n{}\n", shader_name, e);
            }
        }
    }
}

fn generate_embedded_resources() {
    println!("cargo:rerun-if-changed=resources");
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let root = Path::new(&manifest).join("resources");
    let mut paths = Vec::new();
    collect_resource_paths(&root, &root, &mut paths);
    paths.sort();

    let mut source =
        String::from("pub(crate) static EMBEDDED_RESOURCE_FILES: &[EmbeddedResourceFile] = &[\n");
    for path in paths {
        // Debug formatting produces escaped Rust string literals, including on Windows.
        let include_path = format!("/resources/{path}");
        source.push_str(&format!(
            "    EmbeddedResourceFile {{ path: {path:?}, bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), {include_path:?})) }},\n"
        ));
    }
    source.push_str("];\n");
    let out = std::env::var_os("OUT_DIR").expect("OUT_DIR is set");
    fs::write(Path::new(&out).join("embedded_resources.rs"), source)
        .expect("failed to write embedded resource catalog");
}

fn collect_resource_paths(root: &Path, directory: &Path, paths: &mut Vec<String>) {
    for entry in fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "reading resource directory {}: {error}",
            directory.display()
        )
    }) {
        let entry = entry.expect("failed to read resource directory entry");
        let kind = entry
            .file_type()
            .expect("failed to read resource file type");
        let path = entry.path();
        if kind.is_dir() {
            collect_resource_paths(root, &path, paths);
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).expect("resource is within root");
            let key = relative
                .components()
                .map(|part| {
                    part.as_os_str()
                        .to_str()
                        .expect("resource path must be UTF-8")
                })
                .collect::<Vec<_>>()
                .join("/");
            paths.push(key);
        }
    }
}

fn validate_wgsl(source: &str) -> Result<(), String> {
    let module = match naga::front::wgsl::parse_str(source) {
        Ok(m) => m,
        Err(e) => {
            return Err(format!("WGSL Parse Error: {:?}", e));
        }
    };

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );

    match validator.validate(&module) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("WGSL Validation Error: {:?}", e)),
    }
}

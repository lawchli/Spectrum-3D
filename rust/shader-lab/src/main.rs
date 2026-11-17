use std::{env, fs, path::Path};
fn main() {
    let args: Vec<_> = env::args().collect();
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);
    let source = fs::read_to_string(input).unwrap();
    let stage = if input.to_string_lossy().ends_with(".vert") {
        naga::ShaderStage::Vertex
    } else {
        naga::ShaderStage::Fragment
    };
    let mut frontend = naga::front::glsl::Frontend::default();
    let module = match frontend.parse(&naga::front::glsl::Options::from(stage), &source) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", e.emit_to_string(&source));
            std::process::exit(1)
        }
    };
    let info = match naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{}", e.emit_to_string(&source));
            std::process::exit(1)
        }
    };
    let text = naga::back::wgsl::write_string(
        &module,
        &info,
        naga::back::wgsl::WriterFlags::EXPLICIT_TYPES,
    )
    .unwrap();
    if let Some(p) = output.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(output, text).unwrap();
}

# HoloPainter

HoloPainter is a desktop painting application for painting textures directly on 3D models. It is written in Rust, with egui for the UI and wgpu for GPU rendering.

## Demo Video

https://github.com/user-attachments/assets/8b036935-09b6-4bee-95dd-338ce8be1140

## Features

- Texture painting in 3D and UV views
- Brush, fill, selection, decal, and transform tools
- Layers, layer masks, and adjustment layers
- glTF / GLB / FBX model import
- Project saving and loading in the `.holopaint` format
- PNG texture and PSD export

## Building and Running

You need Rust 1.98 or later, Cargo, and a GPU with graphics drivers that support wgpu. To build on Windows, install the MSVC toolchain and Windows SDK.

Run the following command from the repository root:

```sh
cargo run --release
```

To build without launching the application:

```sh
cargo build --release
```

On Windows, the executable is generated at `target/release/holopainter.exe`.

You can also specify a project or model to open at startup:

```sh
cargo run --release -- "path/to/project.holopaint"
cargo run --release -- "path/to/model.glb"
cargo run --release -- --help
```

## Basic Usage

1. Import a glTF / GLB / FBX model to create a new project.
2. Select the material and layer you want to edit, then paint in the 3D or UV view.
3. Save your work as a `.holopaint` project and export PNG textures or PSD files as needed.

## License

HoloPainter's code is licensed under the [MIT License](LICENSE).

The font at `assets/fonts/NotoSansJP-Regular.ttf` is excluded from the MIT License. It is licensed under the SIL Open Font License 1.1 (OFL-1.1). See [assets/fonts/OFL.txt](assets/fonts/OFL.txt) for its copyright notice and full license text.

Dependencies are covered by their respective licenses.

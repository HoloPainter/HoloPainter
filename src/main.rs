use std::process::ExitCode;

use holopainter::{
    app::HoloPainterApp,
    cli::{self, CliAction, StartupRequest},
    ui::workspace::load_startup_workspace,
};

fn main() -> ExitCode {
    match cli::parse_env() {
        Ok(CliAction::PrintHelp) => {
            print!("{}", cli::HELP_TEXT);
            ExitCode::SUCCESS
        }
        Ok(CliAction::PrintVersion) => {
            println!("{}", cli::VERSION_TEXT);
            ExitCode::SUCCESS
        }
        Ok(CliAction::Run(request)) => match run(request) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("Try 'holopainter --help' for more information.");
            ExitCode::from(2)
        }
    }
}

fn run(startup_request: StartupRequest) -> eframe::Result<()> {
    let startup_workspace = load_startup_workspace();
    let icon = image::load_from_memory(holopainter::ui::icons::APP_ICON_PNG_BYTES)
        .expect("embedded application icon must be a valid image")
        .into_rgba8();
    let (icon_width, icon_height) = icon.dimensions();

    let viewport = startup_workspace.apply_to_viewport_builder(
        eframe::egui::ViewportBuilder::default().with_icon(eframe::egui::IconData {
            rgba: icon.into_raw(),
            width: icon_width,
            height: icon_height,
        }),
    );
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        ..Default::default()
    };
    holopainter::native_input::install_event_loop_hook(&mut options);

    eframe::run_native(
        "HoloPainter",
        options,
        Box::new(move |cc| {
            Ok(Box::new(HoloPainterApp::new(
                cc,
                startup_request,
                startup_workspace,
            )))
        }),
    )
}

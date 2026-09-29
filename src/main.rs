use std::process::ExitCode;

use ogma::app::App;

fn main() -> ExitCode {
    
    let result = ratatui::run(|terminal| App::new().run(terminal));

    if let Err(err) = result {
        eprintln!("ogma: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

use std::process::ExitCode;

use ogma::app::App;
use ogma::meta;

fn main() -> ExitCode {
    
    let result = ratatui::run(|terminal| App::new().run(terminal));

    if let Err(err) = result {
        eprintln!("ogma: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Print everything known about one file. A stand-in for the library view.
fn dump_metadata(path: &std::ffi::OsStr) -> ExitCode {
    let file = match meta::probe(path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    println!("codec       {} ({})", file.codec(), file.codec_long());
    println!("container   {:?}", file.container_long());
    println!("lossless    {:?}", file.is_lossless());
    println!("sample rate {:?}", file.sample_rate());
    println!("bit depth   {:?}", file.bits_per_sample());
    println!("channels    {:?} {:?}", file.channel_count(), file.channel_layout());
    println!("duration    {:?}", file.duration());
    println!("bitrate     {:?} kbps", file.audio_bitrate());
    println!("tags        {:?}", file.tag_formats());
    println!("artist      {:?}", file.artist());
    println!("album       {:?}", file.album());
    println!("title       {:?}", file.title());
    println!("track       {:?}/{:?}", file.track_number(), file.track_total());
    println!("year        {:?}", file.year());
    println!("genre       {:?}", file.genre());
    println!("artwork     {} image(s)", file.artwork().len());

    ExitCode::SUCCESS
}

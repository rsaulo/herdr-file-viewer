use std::io::Read;

use herdr_file_viewer::open_target::{self, CliAction};

fn main() -> std::io::Result<()> {
    // Argv parsing lives in the library (`open_target::parse_args`) so it is unit-tested and so
    // unknown / bare flags degrade instead of killing a herdr-spawned pane.
    match open_target::parse_args(std::env::args().skip(1)) {
        CliAction::LaunchDecision => {
            let mut json = String::new();
            std::io::stdin().read_to_string(&mut json)?;
            println!("{}", herdr_file_viewer::launch::launch_decision(&json));
            Ok(())
        }
        CliAction::LaunchDecisionTab => {
            let mut json = String::new();
            std::io::stdin().read_to_string(&mut json)?;
            println!("{}", herdr_file_viewer::launch::launch_decision_tab(&json));
            Ok(())
        }
        CliAction::PrintOpenDirection => {
            // The launcher's one question: which way should herdr split? Resolving it here rather
            // than parsing TOML in bash/PowerShell keeps the lenient-value rules (trim, case,
            // `bottom`) in the one tested place. A missing or malformed config resolves to the
            // default `right`, so the launch is never blocked by a config problem.
            let (config, _) = herdr_file_viewer::config::load_config_from_env();
            let eff = herdr_file_viewer::config::resolve(&config, |k| std::env::var(k).ok());
            println!("{}", eff.open_direction.label());
            Ok(())
        }
        CliAction::Run { open } => herdr_file_viewer::run(open),
    }
}

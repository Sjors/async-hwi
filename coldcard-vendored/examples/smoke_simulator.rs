// Smoke test: connect to running coldcard simulator, fetch xpub.
use coldcard::Coldcard;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path =
        std::env::var("CKCC_SIM_SOCK").unwrap_or_else(|_| "/tmp/ckcc-simulator.sock".to_string());
    eprintln!("connecting to {path}");
    let (mut cc, info) = Coldcard::open_simulator(&path, None)?;
    eprintln!("connected; info.xpub={:?}", info.as_ref().map(|i| &i.xpub));
    let v = cc.version()?;
    println!("version:\n{v}");
    let xpub = cc.xpub(None)?;
    println!("master xpub: {xpub}");
    Ok(())
}

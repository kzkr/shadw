use crate::error::Result;

pub fn exec() -> Result<()> {
    // Stop (ignore "not running" — that's fine)
    let _ = super::stop::exec();

    // Start in background
    super::start::exec(false)
}

use crate::error::Result;

pub fn exec(target: Option<&str>) -> Result<()> {
    // Stop (ignore "not running" — that's fine)
    let _ = super::stop::exec(target);

    // Start in background
    super::start::exec(false, target)
}

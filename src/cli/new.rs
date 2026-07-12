use crate::core::Result;
use crate::scaffold;

pub fn run(opts: scaffold::CliNewOpts) -> Result<()> {
    scaffold::run_new(opts)
}

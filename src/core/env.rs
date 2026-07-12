#[derive(Debug, Clone, Copy)]
pub struct BuildEnvironment {
    pub dev: bool,
    pub source_map: bool,
}

impl BuildEnvironment {
    pub fn current(dev: bool, source_map: bool) -> Self {
        Self { dev, source_map }
    }
}

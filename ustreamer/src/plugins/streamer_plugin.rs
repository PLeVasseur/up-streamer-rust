use std::error::Error;

pub trait StreamerPlugin: Sized + 'static {
    type StartArgs;
    type Instance;
    /// Starts your plugin. Use `Ok` to return your plugin's control structure
    fn start(name: &str, args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>>;
}

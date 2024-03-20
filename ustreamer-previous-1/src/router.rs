use std::error::Error;

pub trait Router: Sized + 'static {
    type StartArgs;
    type Instance;
    /// Starts your router. Use `Ok` to return your router's control structure
    fn start(name: &str, args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>>;
}

use up_rust::uprotocol::UAuthority;

#[derive(Debug)]
pub enum UStreamerError {
    DuplicateTransportTag(crate::ustreamer::TransportTag),
    UAuthorityNotHashable(UAuthority),
    GeneralError(String),
}

impl std::fmt::Display for UStreamerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UStreamerError::DuplicateTransportTag(transport_tag) => {
                write!(f, "Duplicate transport tag found: {}", transport_tag)
            }
            UStreamerError::UAuthorityNotHashable(uauthority) => {
                write!(f, "Unable to has UAuthority: {:?}", uauthority)
            }
            UStreamerError::GeneralError(str) => {
                write!(f, "{}", str)
            }
        }
    }
}

impl std::error::Error for UStreamerError {}

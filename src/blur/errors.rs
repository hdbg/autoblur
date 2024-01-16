use anyhow::Error;

#[derive(Debug, thiserror::Error)]
pub enum BlurInitError {
    #[error("General Failure")]
    General,
    #[error("Browser Failure")]
    BrowserFail(#[from] chromiumoxide::error::CdpError),
    #[error("Can't connect to any of ethereum json rpc nodes")]
    AllEthNodesOffline,
    #[error("Can't verify blur authenthication")]
    AuthNotVerified,
    #[error("Can't parse key")]
    InvalidKey,
    #[error("Invalid page selector")]
    InvalidSelector,
}

#[derive(Debug, thiserror::Error)]
pub enum BlurError {
    #[error("General Failure")]
    GeneralError,
    #[error("Not enough balance")]
    NotEnoughBalance,
    #[error("No Response")]
    NoResponse,
    #[error("Parsing error")]
    ParsingError,
    #[error("Can not find collection")]
    CollectionNotFound,
    #[error("Bid does not exist")]
    BidNotExists
}
pub trait CodedError: std::error::Error {
    fn code(&self) -> &str;
}

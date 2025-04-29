
pub trait CodedError: std::error::Error {
  fn code(&self) -> &str;
}

// // each enum should print to a string with a code of form [B-XXXX]
// impl std::fmt::Display for TaskError {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         let code = match self {
//             TaskError::InvalidInput => "B-0001",
//             TaskError::InvalidOutput => "B-0002",
//             TaskError::InvalidState => "B-0003",
//         };
//         write!(f, "[{}]", code)
//     }
// }

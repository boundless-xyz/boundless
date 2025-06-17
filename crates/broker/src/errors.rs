// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub trait CodedError: std::error::Error {
    fn code(&self) -> &str;
}

// Macro for implementing Debug for CodedError. Ensures the error code is included in the debug output.
#[macro_export]
macro_rules! impl_coded_debug {
    ($name:ident) => {
        use std::backtrace::Backtrace;
        use std::backtrace::BacktraceStatus;
        impl std::fmt::Debug for $name
        where
            $name: CodedError,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let backtrace = Backtrace::capture();
                let code = self.code();
                // If the code is already included in the message, remove it
                let message = self.to_string().replace(code, "");
                write!(f, "{} {} {}", std::any::type_name::<Self>(), code, message)?;
                // Backtrace status == Captured if RUST_BACKTRACE=true
                if backtrace.status() == BacktraceStatus::Captured {
                    write!(f, "\nBacktrace:\n{}", backtrace)?;
                }
                Ok(())
            }
        }
    };
}

pub use impl_coded_debug;

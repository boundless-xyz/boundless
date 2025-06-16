// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

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

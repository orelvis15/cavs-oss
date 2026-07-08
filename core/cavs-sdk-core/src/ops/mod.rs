//! The operation implementations behind the JSON dispatcher. Each module
//! exposes `run(ctx, request_data) -> Result<Value>` where `request_data`
//! is the request's `data` object (schema envelope already stripped).

pub mod analyze;
pub mod apply;
pub mod benchmark;
pub mod pack;
pub mod plan;
pub mod preview;
pub mod savings;
pub mod verify;

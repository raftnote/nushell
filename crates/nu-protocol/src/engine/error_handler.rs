use crate::{record, ShellError, Value, VarId};

use super::{EngineState, Stack};

/// Describes an error handler stored during IR evaluation.
#[derive(Debug, Clone, Copy)]
pub struct ErrorHandler {
    /// Instruction index within the block that will handle the error
    pub handler_index: usize,
    /// Variable to put the error information into, when an error occurs
    pub error_variable: Option<VarId>,
}

impl ErrorHandler {
    /// Add `error_variable` to the stack with the error value.
    pub fn prepare_stack(&self, engine_state: &EngineState, stack: &mut Stack, error: ShellError) {
        if let Some(var_id) = self.error_variable {
            let span = engine_state.get_var(var_id).declaration_span;
            let value = Value::record(
                record! {
                    "msg" => Value::string(format!("{}", error), span),
                    "debug" => Value::string(format!("{:?}", error), span),
                    "raw" => Value::error(error, span),
                },
                span,
            );
            stack.add_var(var_id, value);
        }
    }
}

/// Keeps track of error handlers pushed during evaluation of an IR block.
#[derive(Debug, Clone)]
pub struct ErrorHandlerStack {
    handlers: Vec<ErrorHandler>,
}

impl ErrorHandlerStack {
    pub const fn new() -> ErrorHandlerStack {
        ErrorHandlerStack { handlers: vec![] }
    }

    /// Get the current base of the stack, which establishes a frame.
    pub fn get_base(&self) -> usize {
        self.handlers.len()
    }

    /// Push a new error handler onto the stack.
    pub fn push(&mut self, handler: ErrorHandler) {
        self.handlers.push(handler);
    }

    /// Try to pop an error handler from the stack. Won't go below `base`, to avoid retrieving a
    /// handler belonging to a parent frame.
    pub fn pop(&mut self, base: usize) -> Option<ErrorHandler> {
        if self.handlers.len() > base {
            self.handlers.pop()
        } else {
            None
        }
    }

    /// Reset the stack to the state it was in at the beginning of the frame, in preparation to
    /// return control to the parent frame.
    pub fn leave_frame(&mut self, base: usize) {
        if self.handlers.len() >= base {
            self.handlers.truncate(base);
        } else {
            panic!(
                "ErrorHandlerStack bug: tried to leave frame at {base}, but current base is {}",
                self.get_base()
            )
        }
    }
}

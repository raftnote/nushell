use crate::{ast, ir, DeclId, FromValue, ShellError, Span, Value};

use super::{EngineState, Stack, StateWorkingSet};

/// This is a HACK to help [`Command`](super::Command) support both the old AST evaluator and the
/// new IR evaluator at the same time. It should be removed once we are satisfied with the new
/// evaluator.
#[derive(Debug, Clone)]
pub struct Call<'a> {
    pub head: Span,
    pub decl_id: DeclId,
    pub inner: CallImpl<'a>,
}

#[derive(Debug, Clone)]
pub enum CallImpl<'a> {
    AstRef(&'a ast::Call),
    AstBox(Box<ast::Call>),
    IrRef(&'a ir::Call),
    IrBox(Box<ir::Call>),
}

impl Call<'_> {
    /// Convert the `Call` from any lifetime into `'static`, by cloning the data within onto the
    /// heap.
    pub fn to_owned(&self) -> Call<'static> {
        Call {
            head: self.head,
            decl_id: self.decl_id,
            inner: self.inner.to_owned(),
        }
    }

    /// Assert that the call is `ast::Call`, and fail with an error if it isn't.
    ///
    /// Provided as a stop-gap for commands that can't work with `ir::Call`, or just haven't been
    /// implemented yet. Eventually these issues should be resolved and then this can be removed.
    pub fn assert_ast_call(&self) -> Result<&ast::Call, ShellError> {
        match &self.inner {
            CallImpl::AstRef(call) => Ok(call),
            CallImpl::AstBox(call) => Ok(&call),
            _ => Err(ShellError::NushellFailedSpanned {
                msg: "Can't be used in IR context".into(),
                label: "this command is not yet supported by IR evaluation".into(),
                span: self.head,
            }),
        }
    }

    /// FIXME: implementation asserts `ast::Call` and proxies to that
    pub fn has_flag_const(
        &self,
        working_set: &StateWorkingSet,
        flag_name: &str,
    ) -> Result<bool, ShellError> {
        self.assert_ast_call()?
            .has_flag_const(working_set, flag_name)
    }

    /// FIXME: implementation asserts `ast::Call` and proxies to that
    pub fn get_flag_const<T: FromValue>(
        &self,
        working_set: &StateWorkingSet,
        name: &str,
    ) -> Result<Option<T>, ShellError> {
        self.assert_ast_call()?.get_flag_const(working_set, name)
    }

    /// FIXME: implementation asserts `ast::Call` and proxies to that
    pub fn req_const<T: FromValue>(
        &self,
        working_set: &StateWorkingSet,
        pos: usize,
    ) -> Result<T, ShellError> {
        self.assert_ast_call()?.req_const(working_set, pos)
    }

    /// FIXME: implementation asserts `ast::Call` and proxies to that
    pub fn rest_const<T: FromValue>(
        &self,
        working_set: &StateWorkingSet,
        starting_pos: usize,
    ) -> Result<Vec<T>, ShellError> {
        self.assert_ast_call()?
            .rest_const(working_set, starting_pos)
    }

    /// Returns a span covering the whole call.
    pub fn span(&self) -> Span {
        match &self.inner {
            CallImpl::AstRef(call) => call.span(),
            CallImpl::AstBox(call) => call.span(),
            CallImpl::IrRef(call) => call.span(),
            CallImpl::IrBox(call) => call.span(),
        }
    }

    /// Evaluator-agnostic implementation of `rest_iter_flattened()`. Evaluates or gets all of the
    /// positional and spread arguments, flattens spreads, and then returns one list of values.
    pub fn rest_iter_flattened(
        &self,
        engine_state: &EngineState,
        stack: &mut Stack,
        eval_expression: fn(
            &EngineState,
            &mut Stack,
            &ast::Expression,
        ) -> Result<Value, ShellError>,
        starting_pos: usize,
    ) -> Result<Vec<Value>, ShellError> {
        fn by_ast(
            call: &ast::Call,
            engine_state: &EngineState,
            stack: &mut Stack,
            eval_expression: fn(
                &EngineState,
                &mut Stack,
                &ast::Expression,
            ) -> Result<Value, ShellError>,
            starting_pos: usize,
        ) -> Result<Vec<Value>, ShellError> {
            call.rest_iter_flattened(starting_pos, |expr| {
                eval_expression(engine_state, stack, expr)
            })
        }

        fn by_ir(
            call: &ir::Call,
            stack: &Stack,
            starting_pos: usize,
        ) -> Result<Vec<Value>, ShellError> {
            call.rest_iter_flattened(stack, starting_pos)
        }

        match &self.inner {
            CallImpl::AstRef(call) => {
                by_ast(call, engine_state, stack, eval_expression, starting_pos)
            }
            CallImpl::AstBox(call) => {
                by_ast(call, engine_state, stack, eval_expression, starting_pos)
            }
            CallImpl::IrRef(call) => by_ir(call, stack, starting_pos),
            CallImpl::IrBox(call) => by_ir(call, stack, starting_pos),
        }
    }
}

impl CallImpl<'_> {
    pub fn to_owned(&self) -> CallImpl<'static> {
        match self {
            CallImpl::AstRef(call) => CallImpl::AstBox(Box::new((*call).clone())),
            CallImpl::AstBox(call) => CallImpl::AstBox(call.clone()),
            CallImpl::IrRef(call) => CallImpl::IrBox(Box::new((*call).clone())),
            CallImpl::IrBox(call) => CallImpl::IrBox(call.clone()),
        }
    }
}

impl<'a> From<&'a ast::Call> for Call<'a> {
    fn from(call: &'a ast::Call) -> Self {
        Call {
            head: call.head,
            decl_id: call.decl_id,
            inner: CallImpl::AstRef(call),
        }
    }
}

impl<'a> From<&'a ir::Call> for Call<'a> {
    fn from(call: &'a ir::Call) -> Self {
        Call {
            head: call.head,
            decl_id: call.decl_id,
            inner: CallImpl::IrRef(call),
        }
    }
}

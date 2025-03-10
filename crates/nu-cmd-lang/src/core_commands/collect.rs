use nu_engine::{get_eval_block, redirect_env, CallExt};
use nu_protocol::ast::Call;

use nu_protocol::engine::{Closure, Command, EngineState, Stack};
use nu_protocol::{
    Category, Example, IntoPipelineData, PipelineData, ShellError, Signature, SyntaxShape, Type,
    Value,
};

#[derive(Clone)]
pub struct Collect;

impl Command for Collect {
    fn name(&self) -> &str {
        "collect"
    }

    fn signature(&self) -> Signature {
        Signature::build("collect")
            .input_output_types(vec![(Type::Any, Type::Any)])
            .required(
                "closure",
                SyntaxShape::Closure(Some(vec![SyntaxShape::Any])),
                "The closure to run once the stream is collected.",
            )
            .switch(
                "keep-env",
                "let the block affect environment variables",
                None,
            )
            .category(Category::Filters)
    }

    fn usage(&self) -> &str {
        "Collect the stream and pass it to a block."
    }

    fn run(
        &self,
        engine_state: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let capture_block: Closure = call.req(engine_state, stack, 0)?;

        let block = engine_state.get_block(capture_block.block_id).clone();
        let mut stack_captures = stack.captures_to_stack(capture_block.captures.clone());

        let metadata = input.metadata();
        let input: Value = input.into_value(call.head);

        let mut saved_positional = None;
        if let Some(var) = block.signature.get_positional(0) {
            if let Some(var_id) = &var.var_id {
                stack_captures.add_var(*var_id, input.clone());
                saved_positional = Some(*var_id);
            }
        }

        let eval_block = get_eval_block(engine_state);

        let result = eval_block(
            engine_state,
            &mut stack_captures,
            &block,
            input.into_pipeline_data(),
            call.redirect_stdout,
            call.redirect_stderr,
        )
        .map(|x| x.set_metadata(metadata));

        if call.has_flag(engine_state, stack, "keep-env")? {
            redirect_env(engine_state, stack, &stack_captures);
            // for when we support `data | let x = $in;`
            // remove the variables added earlier
            for (var_id, _) in capture_block.captures {
                stack_captures.remove_var(var_id);
            }
            if let Some(u) = saved_positional {
                stack_captures.remove_var(u);
            }
            // add any new variables to the stack
            stack.vars.extend(stack_captures.vars);
        }
        result
    }

    fn examples(&self) -> Vec<Example> {
        vec![Example {
            description: "Use the second value in the stream",
            example: "[1 2 3] | collect { |x| $x.1 }",
            result: Some(Value::test_int(2)),
        }]
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_examples() {
        use crate::test_examples;

        test_examples(Collect {})
    }
}

use nu_plugin::{EvaluatedCall, LabeledError};
use nu_protocol::{NuString, Value};

pub struct Example;

impl Example {
    pub fn print_values(
        &self,
        index: u32,
        call: &EvaluatedCall,
        input: &Value,
    ) -> Result<(), LabeledError> {
        // Note. When debugging your plugin, you may want to print something to the console
        // Use the eprintln macro to print your messages. Trying to print to stdout will
        // cause a decoding error for your message
        eprintln!("Calling test {index} signature");
        eprintln!("value received {input:?}");

        // To extract the arguments from the Call object you can use the functions req, has_flag,
        // opt, rest, and get_flag
        //
        // Note that plugin calls only accept simple arguments, this means that you can
        // pass to the plug in Int and String. This should be improved when the plugin has
        // the ability to call back to NuShell to extract more information
        // Keep this in mind when designing your plugin signatures
        let a: i64 = call.req(0)?;
        let b: NuString = call.req(1)?;
        let flag = call.has_flag("flag")?;
        let opt: Option<i64> = call.opt(2)?;
        let named: Option<NuString> = call.get_flag("named")?;
        let rest: Vec<NuString> = call.rest(3)?;

        eprintln!("Required values");
        eprintln!("a: {a:}");
        eprintln!("b: {b:}");
        eprintln!("flag: {flag:}");
        eprintln!("rest: {rest:?}");

        if let Some(v) = opt {
            eprintln!("Found optional value opt: {v:}")
        } else {
            eprintln!("No optional value found")
        }

        if let Some(v) = named {
            eprintln!("Named value: {v:?}")
        } else {
            eprintln!("No named value found")
        }

        Ok(())
    }
}

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, parse_quote, ItemFn, Stmt};

fn check_preemption() -> Stmt {
    parse_quote!(check_preemption();)
}

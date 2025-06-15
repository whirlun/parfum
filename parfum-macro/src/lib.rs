use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, ItemFn, parse_macro_input, visit_mut::VisitMut};
use syn::parse_quote;

 fn check_is_check_preemption(call: &syn::ExprCall) -> bool {
     if let syn::Expr::Path(path) = &*call.func {
         path.path.is_ident("check_preemption")
     } else {
         false
     }
 }

struct PreemptionInjector;

impl VisitMut for PreemptionInjector {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // first recurse into sub-expressions
        syn::visit_mut::visit_expr_mut(self, expr);
        // if this is a call not to check_preemption, wrap it
        if let Expr::Call(call_node) = expr {
            if !check_is_check_preemption(call_node) {
                let original = expr.clone();
                *expr = parse_quote!({
                    check_preemption();
                    #original
                });
            }
        }
    }
}

#[proc_macro_attribute]
/// Attribute macro to turn a function into a pseudo-preemptive coroutine
pub fn preemptive(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // parse the function
    let mut input_fn = parse_macro_input!(item as ItemFn);
    // inject preemption checks before each call site throughout the fn
    let mut injector = PreemptionInjector;
    injector.visit_item_fn_mut(&mut input_fn);
    // emit the transformed function
    TokenStream::from(quote!(#input_fn))
}

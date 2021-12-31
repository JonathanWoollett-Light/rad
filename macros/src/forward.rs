use proc_macro::Diagnostic;
use rust_ad_core::traits::*;
use rust_ad_core::Arg;
use rust_ad_core::*;
use std::collections::HashMap;
use syn::spanned::Spanned;

pub fn update_forward_return(s: Option<&mut syn::Stmt>, function_inputs: &[String]) {
    *s.unwrap() = match s {
        Some(syn::Stmt::Semi(syn::Expr::Return(expr_return), _)) => {
            let b = expr_return
                .expr
                .as_ref()
                .expect("update_forward_return: No return expression");
            let expr = &**b;
            let expr_path = expr.path().expect("update_forward_return: No return path");

            let ident = &expr_path.path.segments[0].ident;
            // The if case where `ident == input` is for when you are returning an input.
            let return_str = format!(
                "return ({},{});",
                ident,
                function_inputs
                    .iter()
                    .map(|input| if ident == input {
                        der!(input)
                    } else {
                        wrt!(ident, input)
                    })
                    .intersperse(String::from(","))
                    .collect::<String>()
            );
            syn::parse_str(&return_str).expect("update_forward_return: parse fail")
        }
        _ => panic!("update_forward_return: No return statement:\n{:#?}", s),
    }
}

/// Intersperses values with respect to the preceding values.
pub fn intersperse_succeeding_stmts<K, R>(
    x: Vec<syn::Stmt>,
    extra: K,
    f: fn(&syn::Stmt, &K) -> Result<Option<syn::Stmt>, R>,
) -> Result<Vec<syn::Stmt>, R> {
    let len = x.len();
    let new_len = len * 2 - 1;
    let mut y = Vec::with_capacity(new_len);
    let mut x_iter = x.into_iter().rev();
    if let Some(last) = x_iter.next() {
        y.push(last);
    }
    for a in x_iter {
        let res = f(&a, &extra);
        let opt = match res {
            Ok(r) => r,
            Err(e) => return Err(e),
        };
        if let Some(b) = opt {
            // for c in crate::unwrap_statement(&b).into_iter() {
            //     y.push(c);
            // }
            y.push(b);
        }
        y.push(a);
    }
    Ok(y.into_iter().rev().collect())
}

// TODO Reduce code duplication between `reverse_derivative` and `forward_derivative`
pub fn forward_derivative(
    stmt: &syn::Stmt,
    (type_map, function_inputs): &(&HashMap<String, String>, &[String]),
) -> Result<Option<syn::Stmt>, ()> {
    if let syn::Stmt::Local(local) = stmt {
        let local_ident = local
            .pat
            .ident()
            .expect("forward_derivative: not ident")
            .ident
            .to_string();
        if let Some(init) = &local.init {
            // eprintln!("init: {:#?}",init);
            if let syn::Expr::Binary(bin_expr) = &*init.1 {
                // Creates operation signature struct
                let operation_sig = operation_signature(bin_expr, type_map);
                // Looks up operation with the given lhs type and rhs type and BinOp.
                let operation_out_signature = match SUPPORTED_OPERATIONS.get(&operation_sig) {
                    Some(sig) => sig,
                    None => {
                        let error = format!("unsupported derivative for {}", operation_sig);
                        Diagnostic::spanned(
                            bin_expr.span().unwrap(),
                            proc_macro::Level::Error,
                            error,
                        )
                        .emit();
                        return Err(());
                    }
                };
                // Applies the forward derivative function for the found operation.
                let new_stmt = (operation_out_signature.forward_derivative)(
                    function_inputs,
                    local_ident,
                    &[
                        Arg::try_from(&*bin_expr.left).expect("forward_derivative: bin left"),
                        Arg::try_from(&*bin_expr.right).expect("forward_derivative: bin right"),
                    ],
                );
                return Ok(Some(new_stmt));
            } else if let syn::Expr::Call(call_expr) = &*init.1 {
                // Create function in signature
                let function_in_signature = function_signature(call_expr, type_map);
                // Gets function out signature
                let function_out_signature = match SUPPORTED_FUNCTIONS.get(&function_in_signature) {
                    Some(sig) => sig,
                    None => {
                        let error = format!("unsupported derivative for {}", function_in_signature);
                        Diagnostic::spanned(
                            call_expr.span().unwrap(),
                            proc_macro::Level::Error,
                            error,
                        )
                        .emit();
                        return Err(());
                    }
                };
                let args = call_expr
                    .args
                    .iter()
                    .map(|a| Arg::try_from(a).expect("forward_derivative: call arg"))
                    .collect::<Vec<_>>();
                // Gets new stmt
                let new_stmt = (function_out_signature.forward_derivative)(
                    function_inputs,
                    local_ident,
                    args.as_slice(),
                );

                return Ok(Some(new_stmt));
            } else if let syn::Expr::MethodCall(method_expr) = &*init.1 {
                let method_sig = method_signature(method_expr, type_map);
                let method_out = match SUPPORTED_METHODS.get(&method_sig) {
                    Some(sig) => sig,
                    None => {
                        let error = format!("unsupported derivative for {}", method_sig);
                        Diagnostic::spanned(
                            method_expr.span().unwrap(),
                            proc_macro::Level::Error,
                            error,
                        )
                        .emit();
                        return Err(());
                    }
                };
                let args = {
                    let mut base = Vec::new();
                    let receiver = Arg::try_from(&*method_expr.receiver)
                        .expect("forward_derivative: method receiver");
                    base.push(receiver);
                    let mut args = method_expr
                        .args
                        .iter()
                        .map(|a| Arg::try_from(a).expect("forward_derivative: method arg"))
                        .collect::<Vec<_>>();
                    base.append(&mut args);
                    base
                };

                let new_stmt =
                    (method_out.forward_derivative)(function_inputs, local_ident, args.as_slice());
                return Ok(Some(new_stmt));
            } else if let syn::Expr::Path(expr_path) = &*init.1 {
                // Given `let x = y;`

                // This is `x`
                let out_ident = local
                    .pat
                    .ident()
                    .expect("forward_derivative: not ident")
                    .ident
                    .to_string();
                // This `y`
                let in_ident = expr_path.path.segments[0].ident.to_string();
                // This is type of `y`
                let out_type = type_map
                    .get(&in_ident)
                    .expect("forward_derivative: return not found type");
                let return_type = rust_ad_core::Type::try_from(out_type.as_str())
                    .expect("forward_derivative: unsupported return type");

                let idents = function_inputs
                    .iter()
                    .map(|input| wrt!(out_ident, input))
                    .intersperse(String::from(","))
                    .collect::<String>();
                let derivatives = function_inputs
                    .iter()
                    .map(|input| {
                        cumulative_derivative_wrt_rt(&*init.1, input, function_inputs, &return_type)
                    })
                    .intersperse(String::from(","))
                    .collect::<String>();
                let stmt_str = format!("let ({}) = ({});", idents, derivatives);
                let new_stmt: syn::Stmt =
                    syn::parse_str(&stmt_str).expect("forward_derivative: parse fail");

                return Ok(Some(new_stmt));
            } else if let syn::Expr::Lit(expr_lit) = &*init.1 {
                // Given `let x = y;`

                // This is `x`
                let out_ident = local
                    .pat
                    .ident()
                    .expect("forward_derivative: not ident")
                    .ident
                    .to_string();
                // This is type of `y`
                let out_type = literal_type(expr_lit).expect("forward_derivative: bad lit type");
                let return_type = rust_ad_core::Type::try_from(out_type.as_str())
                    .expect("forward_derivative: unsupported return type");

                let idents = function_inputs
                    .iter()
                    .map(|input| wrt!(out_ident, input))
                    .intersperse(String::from(","))
                    .collect::<String>();
                let derivatives = function_inputs
                    .iter()
                    .map(|input| {
                        cumulative_derivative_wrt_rt(&*init.1, input, function_inputs, &return_type)
                    })
                    .intersperse(String::from(","))
                    .collect::<String>();
                let stmt_str = format!("let ({}) = ({});", idents, derivatives);
                let new_stmt: syn::Stmt =
                    syn::parse_str(&stmt_str).expect("forward_derivative: parse fail");

                return Ok(Some(new_stmt));
            }
        }
    }
    Ok(None)
}

use clang::{token::TokenKind, Accessibility};

use crate::{
    ast_types::SimpleToken,
    utils::{self, FilePathToChangesMap},
    LinterSharedState,
};

const VISIBILITY_FIX_WARNING: &str =
    "Warning: Visibility issues can't be automatically fixed, please fix them manually";

pub fn lint_functions_and_type_declarations(state: &mut LinterSharedState) {
    decl_def_param_names_match(state);
    underscore_suffixed_functions_private(state);

    type_declaration_field_naming(state);

    no_unnecessary_namespace_usages(state);
}

fn underscore_suffixed_functions_private(state: &LinterSharedState) {
    let incorrect_accessability_decls: Vec<_> = state
        .declarations
        .values()
        .filter(|d| d.name.ends_with("_") && d.accessability != Accessibility::Private)
        .collect();
    for decl in incorrect_accessability_decls {
        utils::print_lint_fail_for_location(
            &decl.file_path,
            decl.file_line,
            "Function declaration should be made private since it ends with an underscore",
        );
        if state.auto_fix {
            println!("{VISIBILITY_FIX_WARNING}");
        }
    }
}

fn decl_def_param_names_match(state: &mut LinterSharedState) {
    for (symbol, definition) in state.definitions.iter() {
        let Some(declaration) = state.declarations.get(&symbol.clone()) else {
            continue;
        };

        let decl_params = declaration.params.clone().into_iter();
        let def_params = definition.params.clone().into_iter();

        for (decl_param, def_param) in decl_params.zip(def_params) {
            if decl_param.name == def_param.name {
                continue;
            }
            if decl_param.name.is_empty() {
                utils::print_lint_fail_for_location(
                    &declaration.file_path,
                    declaration.file_line,
                    &format!(
                        "Empty param should be {} to match definition",
                        def_param.name
                    ),
                );
            } else {
                utils::print_lint_fail_for_location(
                    &declaration.file_path,
                    declaration.file_line,
                    &format!(
                        "Param {} should be {} to match definition",
                        decl_param.name, def_param.name
                    ),
                );
            }
            if state.auto_fix {
                let mut replace_with = def_param.name;
                if decl_param.name.is_empty() {
                    replace_with.insert(0, ' ');
                }
                let range_to_change = (
                    decl_param.offset_in_file..decl_param.offset_in_file + decl_param.name.len(),
                    replace_with,
                );
                state
                    .fixes
                    .entry(declaration.file_path.clone())
                    .or_default()
                    .push(range_to_change);
            }
        }
    }
}

fn no_unnecessary_namespace_usages(state: &mut LinterSharedState) {
    for f in state.definitions.values() {
        for ident_token in f.tokens.iter().filter(|t| t.kind == TokenKind::Identifier) {
            check_namespace_usages(
                ident_token,
                &f.namespace,
                &f.file_path,
                &mut state.fixes,
                state.auto_fix,
            );
        }
    }

    for f in state.declarations.values() {
        for ident_token in f.tokens.iter().filter(|t| t.kind == TokenKind::Identifier) {
            check_namespace_usages(
                ident_token,
                &f.namespace,
                &f.file_path,
                &mut state.fixes,
                state.auto_fix,
            );
        }
    }

    for t in state.types.iter() {
        for field in t.fields.iter() {
            for ident_token in field
                .tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Identifier)
            {
                check_namespace_usages(
                    ident_token,
                    &t.namespace,
                    &t.file_path,
                    &mut state.fixes,
                    state.auto_fix,
                );
            }
        }
    }
}

fn check_namespace_usages(
    ident_token: &SimpleToken,
    namespace: &[String],
    file_path: &str,
    changes_map: &mut FilePathToChangesMap,
    auto_fix: bool,
) {
    for nested_namespace in namespace {
        if ident_token.spelling == *nested_namespace {
            utils::print_lint_fail_for_location(
                file_path,
                ident_token.file_line,
                &format!("{}:: should be omitted here", ident_token.spelling),
            );
            if auto_fix {
                let fix = (
                    ident_token.offset_in_file
                        ..ident_token.offset_in_file + ident_token.spelling.len() + 2,
                    String::new(),
                ); // Add 2 for "::"
                changes_map
                    .entry(file_path.to_string())
                    .or_default()
                    .push(fix);
            }
        }
    }
}

fn type_declaration_field_naming(state: &mut LinterSharedState) {
    const OFFSET_VARIABLE_PREFIXES: [&str; 7] =
        ["pad_", "padding_", "field_", "unk_", "gap_", "filler_", "_"];
    const MISC_ALLOWED_PREFIXES: [&str; 5] = ["pad", "unk", "gap", "filler", "unused"];
    const BOOL_ALLOWED_PREFIXES: [&str; 4] = ["is", "has", "should", "always"];
    for type_decl in state.types.iter() {
        for field in type_decl.fields.iter() {
            // Field specific utility closures
            let print_fail_for_field = |warning: &str| {
                utils::print_lint_fail_for_location(&type_decl.file_path, field.file_line, warning)
            };
            let mut add_field_name_fix = |fix: String| {
                if !state.auto_fix {
                    return;
                }
                let fix_change = (
                    field.offset_in_file..field.offset_in_file + field.name.len(),
                    fix,
                );
                state
                    .fixes
                    .entry(type_decl.file_path.clone())
                    .or_default()
                    .push(fix_change);
                println!("Warning: Changed name of field on line {} of file {}, this may cause errors when compiling", field.file_line, &type_decl.file_path);
            };

            let offset_variable_offset_and_prefix = OFFSET_VARIABLE_PREFIXES
                .iter()
                .filter_map(|p| Some((field.name.strip_prefix(p)?.to_string(), p)))
                .next();

            if let Some((offset, prefix)) = offset_variable_offset_and_prefix {
                if offset.bytes().any(|b| b.is_ascii_uppercase()) {
                    print_fail_for_field("Offset variables should be lowercase");
                    add_field_name_fix(format!("{prefix}{}", field.name.to_lowercase()));
                    continue;
                }
                // libclang often fails to get the offset of fields in many cases where the C++ offsetof would probably work (sometimes due to inheritance, sometimes due to non pointer non basic types, etc.), which is why this often won't report all incorect offset variables, but it's better than nothing
                if let Some(actual_offset) = field.offset {
                    if offset != format!("{actual_offset:x}") {
                        print_fail_for_field(&format!(
                            "Offset {offset} does not match actual offset for {}: {actual_offset:x}",
                            field.name
                        ));
                        add_field_name_fix(format!("{prefix}{actual_offset:x}"));
                    }
                }
                continue;
            }

            if MISC_ALLOWED_PREFIXES
                .iter()
                .any(|p| field.name.starts_with(p))
            {
                continue;
            }

            let field_name_no_kind_prefix = if field.is_static {
                field.name.strip_prefix("s").unwrap_or(&field.name)
            } else if !type_decl.is_struct {
                field.name.strip_prefix("m").unwrap_or(&field.name)
            } else {
                &field.name
            };
            let field_name_no_kind_prefix_decapitalized =
                utils::change_str_capitalization(field_name_no_kind_prefix, false);

            // Variable names should always be valid ascii, so indexing this byte array shouldn't
            // cause any problems
            let field_name_bytes = field.name.as_bytes();

            if field.is_static {
                if !field.name.starts_with("s")
                    || field_name_bytes.len() < 2
                    || !field_name_bytes[1].is_ascii_uppercase()
                {
                    print_fail_for_field(
                        "Static fields of classes and structs should be prefixed with `s`",
                    );
                    add_field_name_fix(format!(
                        "s{}",
                        utils::change_str_capitalization(&field.name, true)
                    ));
                    continue;
                }
            }

            if type_decl.is_struct {
                // Skip macro-generated Nerve structs
                if type_decl.name.starts_with("Nrv")
                    || type_decl.name.starts_with("(unnamed struct")
                {
                    continue;
                }

                if field.accessibility != Accessibility::Public {
                    print_fail_for_field("Struct member variables should always be public");
                    if state.auto_fix {
                        println!("{VISIBILITY_FIX_WARNING}");
                    }
                }

                if field_name_bytes[0].is_ascii_uppercase()
                    || (field.name.starts_with("m") && field_name_bytes[1].is_ascii_uppercase())
                {
                    print_fail_for_field(
                        "Member variables of structs should be formatted as noPrefixCamelCase",
                    );
                    add_field_name_fix(field_name_no_kind_prefix_decapitalized.clone());
                }
            } else if !field.is_static {
                if field.accessibility == Accessibility::Public {
                    print_fail_for_field("Class member variables should always be private or protected. Consider using a struct instead if public access is needed");
                    if state.auto_fix {
                        println!("{VISIBILITY_FIX_WARNING}");
                    }
                }

                if !field.name.starts_with("m")
                    || field_name_bytes.len() < 2
                    || !field_name_bytes[1].is_ascii_uppercase()
                {
                    print_fail_for_field("Member variables of classes should be prefixed with `m`");
                    add_field_name_fix(format!(
                        "m{}",
                        utils::change_str_capitalization(&field.name, true)
                    ));
                    continue;
                }
            }
            if field.type_name == "bool"
                && !BOOL_ALLOWED_PREFIXES
                    .iter()
                    .any(|p| field_name_no_kind_prefix_decapitalized.starts_with(p))
            {
                print_fail_for_field("Boolean member variables should be prefixed with (`m`/`s`) `is`, `has` or `always`");
                let field_name_fix = if field.is_static {
                    format!("sIs{}", &field.name[1..])
                } else if type_decl.is_struct {
                    format!("is{}", utils::change_str_capitalization(&field.name, true))
                } else {
                    format!("mIs{}", &field.name[1..])
                };
                add_field_name_fix(field_name_fix);
            }
        }
    }
}

use std::clone;

use clang::{token::TokenKind, Accessibility};

use crate::{
    types::{FilePathToChangesMap, FunctionInfo, LinterSharedState, SimpleToken},
    utils,
};

pub fn lint_functions_and_type_declarations(state: &mut LinterSharedState) {
    underscore_suffixed_functions_private(state);
    override_base_param_names_match(state);
    decl_def_param_names_match(state);
    declaration_overriding_keyword(state);

    type_declaration_field_naming(state);
    type_declaration_field_accessibility(state);

    no_unnecessary_namespace_usages(state);
}

fn underscore_suffixed_functions_private(state: &LinterSharedState) {
    let incorrect_accessibility_decls: Vec<_> = state
        .declarations
        .values()
        .filter(|d| d.name.ends_with("_") && d.accessibility != Accessibility::Private)
        .collect();
    for decl in incorrect_accessibility_decls {
        utils::print_lint_fail_for_location(
            &decl.file_path,
            decl.file_line,
            "Function declaration should be made private since it ends with an underscore, or underscore should be removed from function name",
        );
        if state.auto_fix {
            utils::print_no_visibility_fix_warning();
        }
    }
}

fn override_base_param_names_match(state: &mut LinterSharedState) {
    // The map keys need to be cloned for the borrow checked to be happy with other mutable access
    let decl_symbols: Box<[_]> = state.declarations.keys().cloned().collect();
    for symbol in &decl_symbols {
        let declaration = state.declarations.get(symbol).unwrap();
        if declaration.overriden_method.is_empty() {
            continue;
        }
        let base_method: FunctionInfo = state
            .declarations
            .get(&declaration.overriden_method)
            // Header-implemented virtual functions only appear in the definitions map
            .or_else(|| state.definitions.get(&declaration.overriden_method))
            .cloned()
            .expect(
                "Functions that override other functions should always have a valid base function",
            );
        compare_params(
            &base_method,
            state.declarations.get_mut(symbol).unwrap(),
            &mut state.fixes,
            state.auto_fix,
            "base method",
        );

        if let Some(definition) = state.definitions.get_mut(symbol) {
            compare_params(
                &base_method,
                definition,
                &mut state.fixes,
                state.auto_fix,
                "base method",
            );
        }
    }
}

fn decl_def_param_names_match(state: &mut LinterSharedState) {
    for (symbol, definition) in &state.definitions {
        let Some(declaration) = state.declarations.get_mut(symbol.as_str()) else {
            continue;
        };
        compare_params(
            definition,
            declaration,
            &mut state.fixes,
            state.auto_fix,
            "definition",
        );
    }
}

fn compare_params(
    source_function: &FunctionInfo,
    target_function: &mut FunctionInfo,
    fixes_map: &mut FilePathToChangesMap,
    auto_fix: bool,
    source_type: &str,
) {
    for (source_param, target_param) in source_function
        .params
        .iter()
        .zip(&mut target_function.params)
    {
        if source_param.name == target_param.name {
            continue;
        }
        if source_param.name.is_empty() {
            utils::print_lint_fail_for_location(
                &source_function.file_path,
                source_function.file_line,
                &format!("Params of {source_type} should not be unnamed",),
            );
            if auto_fix {
                utils::print_unable_to_fix_warning("Param needs to be fixed manually");
            }
            continue;
        }
        if target_param.name.is_empty() {
            utils::print_lint_fail_for_location(
                &target_function.file_path,
                target_function.file_line,
                &format!(
                    "Unnamed param should be {} to match {source_type}",
                    source_param.name
                ),
            );
        } else {
            utils::print_lint_fail_for_location(
                &target_function.file_path,
                target_function.file_line,
                &format!(
                    "Param {} should be {} to match {source_type}",
                    target_param.name, source_param.name
                ),
            );
        }
        if auto_fix {
            let mut replace_with = source_param.name.clone();
            let changes = fixes_map
                .entry(target_function.file_path.clone())
                .or_default();
            if target_param.name.is_empty() {
                // Add an extra whitespace for empty param names so that the new param name doesn't
                // become part of the param type
                replace_with.insert(0, ' ');
                changes.push((
                    target_param.offset_in_file..target_param.offset_in_file,
                    replace_with.clone(),
                ));
            } else {
                rename_identifier_tokens(
                    &target_function.tokens,
                    changes,
                    &target_param.name,
                    &replace_with,
                );
            }
            // Update the param name in the shared state so that other lints know about the
            // new value
            target_param.name = replace_with;
        }
    }
}

fn rename_identifier_tokens(
    tokens: &[SimpleToken],
    changes_for_file: &mut Vec<(std::ops::Range<usize>, String)>,
    name: &str,
    new_name: &str,
) {
    for ident in tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Identifier && t.spelling == name)
    {
        let range_to_change = (
            ident.offset_in_file..ident.offset_in_file + name.len(),
            new_name.to_string(),
        );
        changes_for_file.push(range_to_change);
    }
}

fn declaration_overriding_keyword(state: &mut LinterSharedState) {
    for decl in state
        .declarations
        .values()
        .filter(|d| !d.overriden_method.is_empty())
    {
        let Some(virtual_token) = decl.tokens.iter().find(|t| t.spelling == "virtual") else {
            continue;
        };
        utils::print_lint_fail_for_location(
            &decl.file_path,
            virtual_token.file_line,
            "Overriding function declarations should be marked with override and not virtual",
        );
        if state.auto_fix {
            let fixes_for_file = state.fixes.entry(decl.file_path.clone()).or_default();
            let fix = (
                virtual_token.offset_in_file..virtual_token.offset_in_file + "virtual ".len(),
                String::new(),
            );
            fixes_for_file.push(fix);
            utils::print_fix_success();
            if decl.tokens.iter().any(|t| t.spelling == "override") {
                continue;
            }
            let last_closing_parent_index = decl
                .tokens
                .iter()
                .rposition(|t| t.spelling == ")")
                .expect("Function declarations should always have a closing parenthesis");
            let last_const_index = decl.tokens.iter().rposition(|t| t.spelling == "const");
            // If there's also a const keyword, get the index that is bigger, otherwise just return
            // the index of the closing parenthesis
            let max = last_const_index.map_or(last_closing_parent_index, |c| {
                c.max(last_closing_parent_index)
            });
            let index = decl.tokens[max].offset_in_file + "const".len();
            fixes_for_file.push((index..index, " override".to_string()));
        }
    }
}

fn no_unnecessary_namespace_usages(state: &mut LinterSharedState) {
    for f in state.definitions.values() {
        check_namespace_usages(
            &f.tokens,
            &f.namespace,
            &f.file_path,
            &mut state.fixes,
            state.auto_fix,
        );
    }

    for f in state.declarations.values() {
        check_namespace_usages(
            &f.tokens,
            &f.namespace,
            &f.file_path,
            &mut state.fixes,
            state.auto_fix,
        );
    }

    for t in &state.types {
        for field in &t.fields {
            check_namespace_usages(
                &field.tokens,
                &t.namespace,
                &t.file_path,
                &mut state.fixes,
                state.auto_fix,
            );
        }
    }
}

fn check_namespace_usages(
    tokens: &[SimpleToken],
    namespace: &[String],
    file_path: &str,
    changes_map: &mut FilePathToChangesMap,
    auto_fix: bool,
) {
    for (original_i, ident_token) in tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TokenKind::Identifier)
    {
        // Check whether or not the current token is a usage of a namespace the code currently being
        // checked is inside of
        if namespace.contains(&ident_token.spelling)
            && tokens
                .get(original_i + 1)
                .is_some_and(|t| t.kind == TokenKind::Punctuation && t.spelling == "::")
        {
            utils::print_lint_fail_for_location(
                file_path,
                ident_token.file_line,
                &format!("{}:: should be omitted here", ident_token.spelling),
            );
            if auto_fix {
                let fix = (
                    ident_token.offset_in_file
                        ..ident_token.offset_in_file + ident_token.spelling.len() + 2, // Add 2 for "::"
                    String::new(),
                );
                changes_map
                    .entry(file_path.to_string())
                    .or_default()
                    .push(fix);
                utils::print_fix_success();
            }
        }
    }
}

fn type_declaration_field_naming(state: &mut LinterSharedState) {
    const OFFSET_VARIABLE_PREFIXES: [&str; 7] =
        ["pad_", "padding_", "field_", "unk_", "gap_", "filler_", "_"];
    const MISC_ALLOWED_PREFIXES: [&str; 5] = ["pad", "unk", "gap", "filler", "unused"];
    const BOOL_ALLOWED_PREFIXES: [&str; 4] = ["is", "has", "should", "always"];
    for type_decl in &state.types {
        for field in &type_decl.fields {
            // Field specific utility closures
            let mut print_fail_for_field_and_add_fix = |warning: &str, fix: String| {
                utils::print_lint_fail_for_location(&type_decl.file_path, field.file_line, warning);

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
                utils::print_fix_success();
                utils::print_possible_compiler_error_warning_for_line(
                    &type_decl.file_path,
                    field.file_line,
                );
            };

            if let Some(prefix) = OFFSET_VARIABLE_PREFIXES
                .iter()
                .find(|&p| field.name.starts_with(p))
            {
                let offset = &field.name[prefix.len()..];
                if offset.bytes().any(|b| b.is_ascii_uppercase()) {
                    print_fail_for_field_and_add_fix(
                        "Offset variables should be all lowercase",
                        format!("{prefix}{}", field.name.to_lowercase()),
                    );
                    continue;
                }
                // libclang often fails to get the offset of fields even in cases where the C++ offsetof would probably work
                // (sometimes due to inheritance, sometimes due to non pointer non basic types, etc.),
                // which is why this often won't report all incorect offset variables, but it's better than nothing
                if let Some(actual_offset) = field.offset_in_type {
                    if offset != format!("{actual_offset:x}") {
                        print_fail_for_field_and_add_fix(&format!(
                            "Offset {offset} does not match actual offset for {}: {actual_offset:x}",
                            field.name
                        ), format!("{prefix}{actual_offset:x}"));
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

            let field_name_no_m_prefix = field.name.strip_prefix("m").unwrap_or(&field.name);
            let field_name_no_m_prefix_decapitalized =
                utils::change_str_capitalization(field_name_no_m_prefix, false);

            // Variable names should always be valid ascii, so indexing this byte array shouldn't
            // cause any problems
            let field_name_bytes = field.name.as_bytes();

            if type_decl.is_struct {
                // Skip macro-generated Nerve structs
                if type_decl.name.starts_with("Nrv")
                    || type_decl.name.starts_with("(unnamed struct")
                {
                    continue;
                }

                if field_name_bytes[0].is_ascii_uppercase()
                    || (field.name.starts_with("m") && field_name_bytes[1].is_ascii_uppercase())
                {
                    print_fail_for_field_and_add_fix(
                        "Member variables of structs should be formatted as noPrefixCamelCase",
                        field_name_no_m_prefix_decapitalized.clone(),
                    );
                }
            } else if !field.name.starts_with("m")
                || field_name_bytes.len() < 2
                || !field_name_bytes[1].is_ascii_uppercase()
            {
                print_fail_for_field_and_add_fix(
                    "Member variables of classes should be prefixed with `m`",
                    format!("m{}", utils::change_str_capitalization(&field.name, true)),
                );
                continue;
            }
            if field.type_name == "bool"
                && !BOOL_ALLOWED_PREFIXES
                    .iter()
                    .any(|p| field_name_no_m_prefix_decapitalized.starts_with(p))
            {
                let field_name_fix = if type_decl.is_struct {
                    format!("is{}", utils::change_str_capitalization(&field.name, true))
                } else {
                    format!("mIs{}", &field.name[1..])
                };
                print_fail_for_field_and_add_fix("Boolean member variables should be prefixed with (`m`) `is`, `has` or `always`", field_name_fix);
            }
        }
    }
}

fn type_declaration_field_accessibility(state: &mut LinterSharedState) {
    for type_decl in &state.types {
        for field in &type_decl.fields {
            if type_decl.is_struct {
                if field.accessibility != Accessibility::Public {
                    utils::print_lint_fail_for_location(
                        &type_decl.file_path,
                        field.file_line,
                        "Struct member variables should always be public",
                    );
                    if state.auto_fix {
                        utils::print_no_visibility_fix_warning();
                    }
                }
            } else if field.accessibility == Accessibility::Public {
                utils::print_lint_fail_for_location(
                        &type_decl.file_path,
                        field.file_line,
                    "Class member variables should always be private or protected. Consider using a struct instead if public access is needed");
                if state.auto_fix {
                    utils::print_no_visibility_fix_warning();
                }
            }
        }
    }
}

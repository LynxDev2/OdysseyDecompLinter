use clang::{token::TokenKind, Accessibility};

use crate::{
    types::{FilePathToChangesMap, LinterSharedState, SimpleToken},
    utils,
};

pub fn lint_functions_and_type_declarations(state: &mut LinterSharedState) {
    decl_def_param_names_match(state);
    underscore_suffixed_functions_private(state);

    type_declaration_field_naming(state);
    type_declaration_field_accessibility(state);
    no_unnecessary_namespace_usages(state);
}

fn underscore_suffixed_functions_private(state: &LinterSharedState) {
    let incorrect_accessibility_decls: Vec<_> = state
        .declarations
        .values()
        .filter(|d| {
            !d.is_ctor_or_dtor && d.name.ends_with("_") && d.accessibility != Accessibility::Private
        })
        .collect();
    for decl in incorrect_accessibility_decls {
        utils::print_lint_fail_for_location(
            &decl.file_path,
            decl.location.line,
            "Function declaration should be made private since it ends with an underscore, or underscore should be removed from function name",
        );
        if state.auto_fix {
            utils::print_no_visibility_fix_warning();
        }
    }
}

fn decl_def_param_names_match(state: &mut LinterSharedState) {
    for (symbol, definition) in &state.definitions {
        let Some(declaration) = state.declarations.get(symbol.as_str()) else {
            continue;
        };

        for (decl_param, def_param) in declaration.params.iter().zip(&definition.params) {
            if decl_param.name == def_param.name {
                continue;
            }
            if decl_param.name.is_empty() {
                utils::print_lint_fail_for_location(
                    &declaration.file_path,
                    declaration.location.line,
                    &format!(
                        "Empty param should be {} to match definition",
                        def_param.name
                    ),
                );
            } else {
                utils::print_lint_fail_for_location(
                    &declaration.file_path,
                    declaration.location.line,
                    &format!(
                        "Param {} should be {} to match definition",
                        decl_param.name, def_param.name
                    ),
                );
            }
            if state.auto_fix {
                let mut replace_with = def_param.name.clone();
                if decl_param.name.is_empty() {
                    // Add an extra whitespace for empty param names so that the new param name doesn't
                    // become part of the param type
                    replace_with.insert(0, ' ');
                }
                let range_to_change = (
                    decl_param.location.offset..decl_param.location.offset + decl_param.name.len(),
                    replace_with,
                );
                state
                    .fixes
                    .entry(declaration.file_path.clone())
                    .or_default()
                    .push(range_to_change);
                utils::print_fix_success();
            }
        }
    }
}

// Design decision: will also complain about function calls like `A::fun()`.
// This causes problems when `fun()` is virtual, as `fun()` generates an indirect and A::fun() generates a direct branch
fn no_unnecessary_namespace_usages(state: &mut LinterSharedState) {
    for f in state
        .definitions
        .values()
        .chain(state.declarations.values())
        .filter(|f| !f.tokens.is_empty())
    {
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
    if namespace.is_empty() {
        return;
    }
    'tokens: for (original_i, ident_token) in tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TokenKind::Identifier)
    {
        // Allow usage of class name when making function pointers (&A::B) and in function pointer
        // types (A::*)
        if ident_token.spelling == *namespace.last().unwrap()
            && (original_i
                .checked_sub(1)
                .map(|i| &tokens[i])
                .is_some_and(|t| t.spelling == "&")
                || tokens
                    .get(original_i + 2)
                    .is_some_and(|t| t.spelling == "*"))
        {
            continue;
        }

        // Allow the "A::" abd "B::" in "A::B::c() {}"
        let Some(opening_parent_index) = tokens.iter().position(|t| t.spelling == "(") else {
            continue;
        };
        if opening_parent_index < 3 {
            continue;
        }
        let mut j = opening_parent_index - 3;
        if (tokens[j + 1].spelling == "~" || tokens[j + 1].spelling == "operator") && j > 0 {
            j -= 1
        }
        while tokens[j + 1].spelling == "::" {
            if tokens[j].spelling == ident_token.spelling {
                continue 'tokens;
            }
            if j < 2 {
                break;
            }
            j -= 2;
        }

        // Check whether or not the current token is a usage of a namespace the code currently being
        // checked is inside of
        if namespace.contains(&ident_token.spelling)
            && tokens
                .get(original_i + 1)
                .is_some_and(|t| t.kind == TokenKind::Punctuation && t.spelling == "::")
        {
            utils::print_lint_fail_for_location(
                file_path,
                ident_token.location.line,
                &format!("{}:: should be omitted here", ident_token.spelling),
            );
            if auto_fix {
                let fix = (
                    ident_token.location.offset
                        ..ident_token.location.offset + ident_token.spelling.len() + 2, // Add 2 for "::"
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
    const BOOL_ALLOWED_PREFIXES: [&str; 5] = ["is", "has", "should", "always", "value"];
    for type_decl in &state.types {
        for field in &type_decl.fields {
            // Field specific utility closures
            let mut print_fail_for_field_and_add_fix = |warning: &str, fix: String| {
                utils::print_lint_fail_for_location(
                    &type_decl.file_path,
                    field.location.line,
                    warning,
                );

                if !state.auto_fix {
                    return;
                }
                let fix_change = (
                    field.location.offset..field.location.offset + field.name.len(),
                    fix.clone(),
                );
                state
                    .fixes
                    .entry(type_decl.file_path.clone())
                    .or_default()
                    .push(fix_change);
                utils::print_fix_success();
                if type_decl.is_struct {
                    utils::print_possible_compiler_error_warning_for_line(
                        &type_decl.file_path,
                        field.location.line,
                    );
                }
                for member_fn_def in state
                    .definitions
                    .values()
                    .filter(|d| d.namespace.last().is_some_and(|n| n == &type_decl.name))
                {
                    rename_identifier_tokens(
                        &member_fn_def.tokens,
                        state
                            .fixes
                            .entry(member_fn_def.file_path.clone())
                            .or_default(),
                        &field.name,
                        &fix,
                    );
                }
            };

            if let Some(prefix) = OFFSET_VARIABLE_PREFIXES
                .iter()
                .find(|&p| field.name.starts_with(p))
            {
                let offset = &field.name[prefix.len()..];
                if offset.bytes().any(|b| b.is_ascii_uppercase()) {
                    print_fail_for_field_and_add_fix(
                        "Offset variables should be all lowercase",
                        format!("{prefix}{}", offset.to_lowercase()),
                    );
                    continue;
                }
                // Offset is None when a template of the parent type affects its size in memory
                // (it contains field(s) that have a type from a generic parameter)
                if let Some(actual_offset) = field.offset_in_type {
                    if offset != format!("{actual_offset:x}") {
                        print_fail_for_field_and_add_fix(&format!(
                            "Offset {offset} does not match actual offset for {}: {actual_offset:x}",
                            field.name
                        ), format!("{prefix}{actual_offset:x}"));
                    }
                } else {
                    utils::print_lint_fail_for_location(
                        &type_decl.file_path,
                        field.location.line,
                        "Unable to calculate offset for field, which likely means it is part of a templated type and its offset should not be assumed",
                    );
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
                // SMO: Skip macro-generated Nerve structs
                if type_decl.name.starts_with("Nrv")
                    || type_decl.name.starts_with("(unnamed struct")
                {
                    continue;
                }

                if field_name_bytes[0].is_ascii_uppercase()
                    || (field.name.starts_with("m")
                        && field_name_bytes
                            .get(1)
                            .is_some_and(|c| c.is_ascii_uppercase()))
                {
                    print_fail_for_field_and_add_fix(
                        "Member variables of structs should be formatted as noPrefixCamelCase",
                        field_name_no_m_prefix_decapitalized.clone(),
                    );
                    continue;
                }
            } else if !field.name.starts_with("m")
                || field_name_bytes
                    .get(1)
                    .is_none_or(|c| c.is_ascii_lowercase())
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
                print_fail_for_field_and_add_fix("Boolean member variables should be prefixed with (`m`) `is`, `has`, `should`, or `always`", field_name_fix);
            }
        }
    }
}

// does not handle variable shadowing (renames everything matching given identifier)
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
            ident.location.offset..ident.location.offset + name.len(),
            new_name.to_string(),
        );
        changes_for_file.push(range_to_change);
    }
}

fn type_declaration_field_accessibility(state: &mut LinterSharedState) {
    for type_decl in &state.types {
        for field in &type_decl.fields {
            if type_decl.is_struct {
                if field.accessibility != Accessibility::Public {
                    utils::print_lint_fail_for_location(
                        &type_decl.file_path,
                        field.location.line,
                        "Struct member variables should always be public",
                    );
                    if state.auto_fix {
                        utils::print_no_visibility_fix_warning();
                    }
                }
            } else if field.accessibility == Accessibility::Public {
                utils::print_lint_fail_for_location(
                        &type_decl.file_path,
                        field.location.line,
                    "Class member variables should always be private or protected. Consider using a struct instead if public access is needed");
                if state.auto_fix {
                    utils::print_no_visibility_fix_warning();
                }
            }
        }
    }
}

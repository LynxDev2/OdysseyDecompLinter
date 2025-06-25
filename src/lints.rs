use clang::Accessibility;

use crate::{utils, LinterSharedState};

const VISIBILITY_FIX_WARNING: &str =
    "Warning: Visibility issues can't be automatically fixed, please fix them manually";

pub fn lint_functions(state: &mut LinterSharedState) {
    decl_def_param_names_match(state);
    underscore_suffixed_functions_private(state);
}

pub fn lint_type_definitions(state: &mut LinterSharedState) {
    type_declaration_field_naming(state);
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
        let declaration = state.declarations.get(&symbol.clone());
        if declaration.is_none() {
            continue;
        }

        let declaration = declaration.unwrap();

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
                    add_field_name_fix(field_name_no_m_prefix_decapitalized.clone());
                }
            } else {
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
                    .any(|p| field_name_no_m_prefix_decapitalized.starts_with(p))
            {
                print_fail_for_field("Boolean member variables should be prefixed with (`m`) `is`, `has` or `always`");
                let field_name_fix = if type_decl.is_struct {
                    format!("is{}", utils::change_str_capitalization(&field.name, true))
                } else {
                    format!("mIs{}", &field.name[1..])
                };
                add_field_name_fix(field_name_fix);
            }
        }
    }
}

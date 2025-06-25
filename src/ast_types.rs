/// This file contains types that represent AST entities. All types in this file are constructed
/// from clang::Entity objects. The main advantages of these types are that they give easy access
/// to all fields from specific kinds of entities that we need and unlike clang::Entity objects,
/// objects of these types aren't tied to the lifetime of the TU in any way
use clang::Accessibility;
use clang::EntityKind::*;
use std::hash::Hash;

#[derive(Clone, Debug)]
pub struct FunctionInfo {
    pub name: String,
    pub is_declaration: bool,
    pub file_path: String,
    pub file_line: u32,
    pub params: Vec<FunctionParam>,
    pub accessability: Accessibility,
}

impl FunctionInfo {
    pub fn new(function_entity: &clang::Entity) -> FunctionInfo {
        assert!(
            matches!(
                function_entity.get_kind(),
                FunctionDecl | Method | Constructor
            ),
            "Function entity should be of type FunctionDecl, Method or Constructor"
        );
        let is_declaration = !function_entity.is_definition();
        let loc = function_entity
            .get_location()
            .expect("Function entities should always have a valid location")
            .get_file_location();
        let file_path = loc
            .file
            .expect("Function entity locations should always have a valid file name")
            .get_path()
            .to_str()
            .expect("Function entity location file paths should always be valid as strs")
            .to_string();
        let params: Vec<_> = function_entity
            .get_arguments()
            .unwrap_or_default()
            .into_iter()
            .map(|a| FunctionParam::new(&a))
            .collect();
        let name = function_entity
            .get_name()
            .expect("Function entities should always have a name");
        FunctionInfo {
            name,
            is_declaration,
            file_path,
            file_line: loc.line,
            params,
            accessability: function_entity
                .get_accessibility()
                .unwrap_or(Accessibility::Private),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FunctionParam {
    pub name: String,
    pub offset_in_file: usize,
    // TODO: Store default value
}

impl FunctionParam {
    pub fn new(param_entity: &clang::Entity) -> FunctionParam {
        FunctionParam {
            name: param_entity.get_name().unwrap_or_default(),
            offset_in_file: param_entity
                .get_location()
                .expect("Param entities should always have a location")
                .get_file_location()
                .offset as usize,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeDeclaration {
    pub name: String,
    pub is_struct: bool,
    pub file_path: String,
    pub fields: Vec<TypeField>,
}

impl TypeDeclaration {
    pub fn new(type_decl_entity: &clang::Entity) -> TypeDeclaration {
        assert!(
            matches!(type_decl_entity.get_kind(), StructDecl | ClassDecl),
            "Type declaration entity should be of type StructDecl or ClassDecl"
        );
        let loc = type_decl_entity
            .get_location()
            .expect("Type declaration entities should always have a valid location")
            .get_file_location();
        let file_path = loc
            .file
            .expect("Type declaration entity locations should always have a valid file name")
            .get_path()
            .to_str()
            .expect("Type declaration entity location file paths should always be valid as strs")
            .to_string();
        let fields: Vec<_> = type_decl_entity
            .get_children()
            .iter()
            .filter_map(|c| {
                if c.get_kind() != FieldDecl {
                    return None;
                }
                Some(TypeField::new(c))
            })
            .collect();
        TypeDeclaration {
            name: type_decl_entity.get_name().unwrap_or_default(),
            is_struct: type_decl_entity.get_kind() == StructDecl,
            file_path,
            fields,
        }
    }
}

impl PartialEq for TypeDeclaration {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for TypeDeclaration {}

impl Hash for TypeDeclaration {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct TypeField {
    pub name: String,
    pub type_name: String,
    pub offset_in_file: usize,
    pub file_line: u32,
    pub accessibility: Accessibility,
    pub offset: Option<usize>,
}

impl TypeField {
    pub fn new(field_entity: &clang::Entity) -> TypeField {
        let name = field_entity.get_name().unwrap_or_default();
        TypeField {
            name,
            type_name: field_entity
                .get_type()
                .expect("Type field entites should always have a valid internal type field")
                .get_display_name(),
            offset_in_file: field_entity
                .get_location()
                .expect("Type field entities should always have a location")
                .get_file_location()
                .offset as usize,
            accessibility: field_entity
                .get_accessibility()
                .unwrap_or(Accessibility::Private),
            file_line: field_entity
                .get_location()
                .expect("Param entities should always have a location")
                .get_file_location()
                .line,
            offset: field_entity
                .get_offset_of_field()
                .ok()
                // Convert number of bits from object start into decimal
                .map(|o| o / 8),
        }
    }
}

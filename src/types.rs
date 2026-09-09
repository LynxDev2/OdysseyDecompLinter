use clang::token::TokenKind;
use clang::Accessibility;
use clang::EntityKind::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;

pub type SymbolToFunctionInfoMap = HashMap<String, FunctionInfo>;

/// Used to store the changes to files that should be applied once all checks have been completed.
/// These can't be strings that are directly changed because the file data libclang points to
/// wouldn't change causing there to be an index mismatch for the next change
pub type FilePathToChangesMap = HashMap<String, Vec<(std::ops::Range<usize>, String)>>;

pub struct LinterSharedState {
    pub definitions: SymbolToFunctionInfoMap,
    pub declarations: SymbolToFunctionInfoMap,
    pub types: HashSet<TypeDeclaration>,
    pub fixes: FilePathToChangesMap,
    pub auto_fix: bool,
}

impl LinterSharedState {
    pub fn new(
        declarations: SymbolToFunctionInfoMap,
        definitions: SymbolToFunctionInfoMap,
        types: HashSet<TypeDeclaration>,
        auto_fix: bool,
    ) -> LinterSharedState {
        LinterSharedState {
            definitions,
            declarations,
            types,
            fixes: FilePathToChangesMap::new(),
            auto_fix,
        }
    }
}

/// These types represent AST entities and tokens. All of these types are constructed
/// from clang objects. The main advantages of these types are that they give easy access
/// to all fields from specific kinds of entities that we need and unlike clang objects,
/// objects of these types aren't tied to the lifetime of the TU in any way

#[derive(Clone, Debug)]
pub struct LocationInFile {
    pub line: u32,
    pub offset: usize,
}

impl From<clang::source::Location<'_>> for LocationInFile {
    fn from(loc: clang::source::Location<'_>) -> Self {
        LocationInFile {
            line: loc.line,
            offset: loc.offset as usize,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FunctionInfo {
    pub name: String,
    pub file_path: String,
    pub location: LocationInFile,
    pub params: Vec<FunctionParam>,
    pub accessibility: Accessibility,
    pub tokens: Vec<SimpleToken>,
    pub namespace: Vec<String>,
    pub is_ctor_or_dtor: bool,
    pub params_paren_offset: Option<usize>,
}

impl FunctionInfo {
    pub fn new(function_entity: &clang::Entity, mut namespace: Vec<String>) -> FunctionInfo {
        assert!(
            matches!(
                function_entity.get_kind(),
                FunctionDecl | Method | Constructor | Destructor | ConversionFunction | FunctionTemplate
            ),
            "Function entity should be of type FunctionDecl, Method, Constructor, Destructor, ConversionFunction or FunctionTemplate"
        );
        let loc = function_entity
            .get_location()
            .expect("Function entities should always have a valid location")
            .get_file_location();
        let file_path = loc
            .file
            .expect("Function entity locations should always have a valid file object")
            .get_path()
            .to_str()
            .expect("Function entity location file paths should always be valid as strs")
            .to_string();
        let params: Vec<_> = function_entity
            .get_arguments()
            .unwrap_or_default()
            .iter()
            .map(FunctionParam::new)
            .collect();
        let name = function_entity
            .get_name()
            .expect("Function entities should always have a name");
        let range = function_entity
            .get_range()
            .expect("Function entites should always have a valid source range");
        let accessibility = function_entity
            .get_accessibility()
            // TODO: better logic to capture default on namespace and cpp-only functions
            .unwrap_or(Accessibility::Private);
        let tokens: Vec<_> = range.tokenize().iter().map(SimpleToken::new).collect();
        let lexical_parent_name = function_entity
            .get_lexical_parent()
            .expect("Function entities should always have a lexical parent")
            .get_name()
            .unwrap_or_default();
        let mut inline_namespace = Vec::new();
        let mut semantic_parent = function_entity.get_semantic_parent();
        // Add any semantic parents that aren't lexical parents (like "A" and "B" in "A::B::C() {}" to the namespace)
        while let Some(parent) = semantic_parent {
            let Some(name) = parent.get_name() else {
                // anonymous namespaces
                break;
            };
            if name == lexical_parent_name {
                // all "outer ones" should be listed in namespace already
                break;
            }
            inline_namespace.push(name);
            semantic_parent = parent.get_semantic_parent();
        }
        namespace.extend(inline_namespace.into_iter().rev());
        // Get the offset of the paren starting the parameter list. This stored so that the
        // namespace lint can skip checking the namespaces of return types
        let params_paren_offset = function_entity
            .get_name_ranges()
            .last()
            .map(|r| r.get_end().get_file_location().offset as usize)
            .and_then(|name_end_offset| {
                tokens
                    .iter()
                    .find(|t| {
                        t.kind == TokenKind::Punctuation
                            && t.spelling == "("
                            && t.location.offset >= name_end_offset
                    })
                    .map(|t| t.location.offset)
            });
        FunctionInfo {
            name,
            file_path,
            location: loc.into(),
            params,
            accessibility,
            tokens,
            namespace,
            is_ctor_or_dtor: matches!(function_entity.get_kind(), Constructor | Destructor),
            params_paren_offset,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FunctionParam {
    pub name: String,
    pub location: LocationInFile,
    // TODO: Store default value for OdysseyDecomp#495
}

impl FunctionParam {
    pub fn new(param_entity: &clang::Entity) -> FunctionParam {
        FunctionParam {
            name: param_entity.get_name().unwrap_or_default(),
            location: param_entity
                .get_location()
                .expect("Param entities should always have a valid location")
                .get_file_location()
                .into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeDeclaration {
    pub name: String,
    pub is_struct: bool, // true = struct, false = class
    pub file_path: String,
    pub fields: Vec<TypeField>,
    pub namespace: Vec<String>,
}

impl TypeDeclaration {
    pub fn new(type_decl_entity: &clang::Entity, namespace: Vec<String>) -> TypeDeclaration {
        assert!(
            matches!(type_decl_entity.get_kind(), StructDecl | ClassDecl | ClassTemplate | ClassTemplatePartialSpecialization),
            "Type declaration entity should be of type StructDecl, ClassDecl, ClassTemplate or ClassTemplatePartialSpecialization"
        );
        let name = type_decl_entity
            .get_name()
            .expect("Type declaration entities should always have a valid name");
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
            .filter(|c| c.get_kind() == FieldDecl)
            .map(TypeField::new)
            .collect();
        let is_templated_struct = matches!(
            type_decl_entity.get_kind(),
            ClassTemplate | ClassTemplatePartialSpecialization
        ) && type_decl_entity
            .get_template_kind()
            .expect("Template entites should always have a template kind")
            == StructDecl;
        TypeDeclaration {
            name,
            is_struct: type_decl_entity.get_kind() == StructDecl || is_templated_struct,
            file_path,
            fields,
            namespace,
        }
    }
}

// These three traits are implemented to allow for HashSets of this type
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
    pub location: LocationInFile,
    pub accessibility: Accessibility,
    pub offset_in_type: Option<usize>,
    pub tokens: Vec<SimpleToken>,
}

impl TypeField {
    pub fn new(field_entity: &clang::Entity) -> TypeField {
        let name = field_entity
            .get_name()
            .expect("Type field entities should always have a valid name");
        let loc = field_entity
            .get_location()
            .expect("Type field entities should always have a valid location")
            .get_file_location();
        let range = field_entity
            .get_range()
            .expect("Type field entites should always a valid source range");
        let tokens: Vec<_> = range.tokenize().iter().map(SimpleToken::new).collect();
        TypeField {
            name,
            type_name: field_entity
                .get_type()
                .expect("Type field entites should always have a valid internal type field")
                .get_display_name(),
            location: loc.into(),
            accessibility: field_entity
                .get_accessibility()
                .expect("Type field entites should always have a valid accessibility field"),
            offset_in_type: field_entity
                .get_offset_of_field()
                .ok()
                // Convert number of bits from type start into decimal
                .map(|o| o / 8),
            tokens,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimpleToken {
    pub kind: TokenKind,
    pub spelling: String,
    pub location: LocationInFile,
}

impl SimpleToken {
    pub fn new(clang_token: &clang::token::Token) -> SimpleToken {
        SimpleToken {
            kind: clang_token.get_kind(),
            spelling: clang_token.get_spelling(),
            location: clang_token.get_location().get_file_location().into(),
        }
    }
}

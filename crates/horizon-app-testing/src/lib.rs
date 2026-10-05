#![forbid(unsafe_code)]

pub mod catalog;
pub mod contract;
pub mod recipe;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("device_contract_missing: no remote-device-testing YAML block was found")]
    ContractMissing,
    #[error("device_contract_invalid: the remote-device-testing contract is invalid")]
    ContractInvalid,
    #[error("device_path_rejected: the path must stay inside the selected repository")]
    PathRejected,
    #[error("device_recipe_invalid: the recipe has invalid or unsupported steps")]
    RecipeInvalid,
    #[error("device_catalog_invalid: the provider returned an invalid native-device catalog")]
    CatalogInvalid,
    #[error("device_matrix_unavailable: no offered device matches a matrix entry")]
    MatrixUnavailable,
    #[error("device_file_unavailable: a required project file is unavailable")]
    FileUnavailable,
}

pub type Result<T> = std::result::Result<T, Error>;

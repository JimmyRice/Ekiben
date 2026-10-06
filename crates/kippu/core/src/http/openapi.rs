//! The OpenAPI document's frame: title, shared schemas and the bearer scheme.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::error::Problem;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Kippu",
        description = "Ticketing infrastructure for conventions. Errors are RFC 9457 problems."
    ),
    components(schemas(Problem)),
    modifiers(&BearerAuth)
)]
pub(crate) struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

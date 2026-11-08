//! Google Cloud over REST.
//!
//! Horizon reaches Google's APIs from inside the process and never runs `gcloud`.
//! A downloaded service-account key holds everything needed: Horizon signs a JWT
//! with the key's RSA private key and trades it for an `OAuth2` access token, which
//! its requests then carry as a bearer token.

pub mod token;

fn main() -> Result<(), serde_json::Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(&horizon_app_testing::contract::schema())?
    );
    Ok(())
}

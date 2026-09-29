use serialem_client::{CommandResult, SerialEmClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = SerialEmClient::connect_default()?;
    let result = client.ReportMag()?;
    match result {
        CommandResult::Number(magnification) => println!("Current magnification: {magnification}"),
        other => println!("ReportMag result: {other:?}"),
    }
    Ok(())
}

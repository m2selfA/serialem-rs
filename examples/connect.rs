use serialem_client::SerialEmClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = SerialEmClient::connect_default()?;
    println!("connected to {}", client.address());
    println!(
        "upstream module count: {}",
        client.report_num_module_funcs()
    );
    println!(
        "external wrapper count: {}",
        client.report_num_external_funcs()
    );
    println!("ready: {}", client.ok_to_run_external_script()?);
    Ok(())
}

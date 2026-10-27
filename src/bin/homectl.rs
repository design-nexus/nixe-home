use nyx_home::automation::{BUS, DryRunContext, INTERFACE, Owner, PATH, Workflow};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|value| value == "dry-run") && (args.len() == 2 || args.len() == 3)
    {
        let workflow: Workflow = serde_json::from_str(&std::fs::read_to_string(&args[1])?)?;
        let context: DryRunContext = if args.len() == 3 {
            serde_json::from_str(&std::fs::read_to_string(&args[2])?)?
        } else {
            DryRunContext::default()
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&workflow.dry_run(&context)?)?
        );
        return Ok(());
    }
    let connection = zbus::Connection::session().await?;
    let proxy = zbus::Proxy::new(&connection, BUS, PATH, INTERFACE).await?;
    match args.first().map(String::as_str) {
        Some("status") => println!("{}", proxy.call::<_, _, String>("Status", &()).await?),
        Some("list") => println!(
            "{}",
            proxy.call::<_, _, String>("ListWorkflows", &()).await?
        ),
        Some("history") => println!(
            "{}",
            proxy.call::<_, _, String>("History", &(100u32)).await?
        ),
        Some("save") if args.len() == 2 => {
            let json = std::fs::read_to_string(&args[1])?;
            let workflow: Workflow = serde_json::from_str(&json)?;
            workflow.validate()?;
            if workflow.owner == Owner::HomeAssistant {
                return Err(
                    "Home Assistant workflows must be saved through the Home editor".into(),
                );
            }
            println!(
                "{}",
                proxy.call::<_, _, String>("SaveWorkflow", &json).await?
            );
        }
        Some("run") if args.len() == 2 => println!(
            "{}",
            proxy.call::<_, _, String>("RunWorkflow", &args[1]).await?
        ),
        Some("enable") if args.len() == 2 => {
            proxy
                .call::<_, _, ()>("SetEnabled", &(&args[1], true))
                .await?;
        }
        Some("disable") if args.len() == 2 => {
            proxy
                .call::<_, _, ()>("SetEnabled", &(&args[1], false))
                .await?;
        }
        Some("delete") if args.len() == 2 => println!(
            "{}",
            proxy.call::<_, _, bool>("DeleteWorkflow", &args[1]).await?
        ),
        _ => {
            eprintln!(
                "Usage: nyx-homectl status|list|history|save FILE|dry-run FILE [CONTEXT]|run ID|enable ID|disable ID|delete ID"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}

use crate::commands::error::CommandError;
use crate::wallet;
use zcash_primitives::memo::MemoBytes;

pub(super) fn parse_send_args(
    args: &[&str],
) -> Result<Vec<(String, u64, Option<MemoBytes>)>, CommandError> {
    // Check for a single argument that can be parsed as JSON
    let send_args = if args.len() == 1 {
        let json_args = json::parse(args[0]).map_err(CommandError::FailedJsonParsing)?;

        if !json_args.is_array() {
            return Err(CommandError::UnexpectedType);
        }

        json_args
            .members()
            .map(|j| {
                if !j.has_key("address") {
                    return Err(CommandError::MissingKey("address".to_string()));
                } else if !j.has_key("amount") {
                    return Err(CommandError::MissingKey("amount".to_string()));
                }

                let address = j["address"]
                    .as_str()
                    .ok_or(CommandError::UnexpectedType)?
                    .to_string();
                let amount = j["amount"].as_u64().ok_or(CommandError::UnexpectedType)?;
                let memo = if let Some(m) = j["memo"].as_str().map(|s| s.to_string()) {
                    Some(
                        wallet::utils::interpret_memo_string(m)
                            .map_err(CommandError::InvalidMemo)?,
                    )
                } else {
                    None
                };

                Ok((address, amount, memo))
            })
            .collect::<Result<Vec<(String, u64, Option<MemoBytes>)>, CommandError>>()
    } else if args.len() == 2 || args.len() == 3 {
        let address = args[0].to_string();
        let amount = args[1]
            .parse::<u64>()
            .map_err(CommandError::FailedIntParsing)?;
        let memo = if args.len() == 3 {
            Some(
                wallet::utils::interpret_memo_string(args[2].to_string())
                    .map_err(CommandError::InvalidMemo)?,
            )
        } else {
            None
        };

        Ok(vec![(address, amount, memo)])
    } else {
        return Err(CommandError::InvalidArguments);
    }?;

    Ok(send_args)
}

//! Module containing utility functions for the commands interface

use json::JsonValue;

use zcash_address::ZcashAddress;
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::Zatoshis;

use crate::commands::error::CommandError;
use zingolib::data::receivers::Receivers;
use zingolib::utils::conversion::{address_from_str, zatoshis_from_u64};
use zingolib::wallet;

// Parse the send arguments for `do_send`.
// The send arguments have two possible formats:
// - 1 argument in the form of a JSON string for multiple sends. '[{"address":"<address>", "value":<value>, "memo":"<optional memo>"}, ...]'
// - 2 (+1 optional) arguments for a single address send. &["<address>", <amount>, "<optional memo>"]
pub(super) fn parse_send_args(args: &[&str]) -> Result<Receivers, CommandError> {
    // Check for a single argument that can be parsed as JSON
    let send_args = if args.len() == 1 {
        let json_args = json::parse(args[0]).map_err(CommandError::ArgsNotJson)?;

        if !json_args.is_array() {
            return Err(CommandError::SingleArgNotJsonArray(json_args.to_string()));
        }
        if json_args.is_empty() {
            return Err(CommandError::EmptyJsonArray);
        }

        json_args
            .members()
            .map(|j| {
                let recipient_address = address_from_json(j)?;
                let amount = zatoshis_from_json(j)?;
                let memo = memo_from_json(j)?;
                check_memo_compatibility(&recipient_address, &memo)?;

                Ok(zingolib::data::receivers::Receiver {
                    recipient_address,
                    amount,
                    memo,
                })
            })
            .collect::<Result<Receivers, CommandError>>()
    } else if args.len() == 2 || args.len() == 3 {
        let recipient_address =
            address_from_str(args[0]).map_err(CommandError::ConversionFailed)?;
        let amount_u64 = args[1]
            .trim()
            .parse::<u64>()
            .map_err(CommandError::ParseIntFromString)?;
        let amount = zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)?;
        let memo = if args.len() == 3 {
            Some(
                wallet::utils::memo_bytes_from_string(args[2].to_string())
                    .map_err(CommandError::InvalidMemo)?,
            )
        } else {
            None
        };
        check_memo_compatibility(&recipient_address, &memo)?;

        Ok(vec![zingolib::data::receivers::Receiver {
            recipient_address,
            amount,
            memo,
        }])
    } else {
        return Err(CommandError::InvalidArguments);
    }?;

    Ok(send_args)
}

// The send arguments have two possible formats:
// - 1 arguments in the form of:
//    *  a JSON string (single address only). '[{"address":"<address>", "memo":"<optional memo>"}]'
// - 1 + 1 optional arguments for a single address send. &["<address>", "<optional memo>"]
pub(super) fn parse_send_all_args(
    args: &[&str],
) -> Result<(ZcashAddress, Option<MemoBytes>), CommandError> {
    let address: ZcashAddress;
    let memo: Option<MemoBytes>;
    if args.len() == 1 {
        if let Ok(addr) = address_from_str(args[0]) {
            address = addr;
            memo = None;
            check_memo_compatibility(&address, &memo)?;
        } else {
            let json_arg =
                json::parse(args[0]).map_err(|_e| CommandError::ArgNotJsonOrValidAddress)?;
            if json_arg.is_array() {
                return Err(CommandError::JsonArrayNotObj(json_arg.to_string()));
            }
            if json_arg.is_empty() {
                return Err(CommandError::EmptyJsonArray);
            }
            address = address_from_json(&json_arg)?;
            memo = memo_from_json(&json_arg)?;
            check_memo_compatibility(&address, &memo)?;
        }
    } else if args.len() == 2 {
        address = address_from_str(args[0]).map_err(CommandError::ConversionFailed)?;
        memo = Some(
            wallet::utils::memo_bytes_from_string(args[1].to_string())
                .map_err(CommandError::InvalidMemo)?,
        );
        check_memo_compatibility(&address, &memo)?;
    } else {
        return Err(CommandError::InvalidArguments);
    }
    Ok((address, memo))
}

// Parse the arguments for `spendable_balance`.
// The arguments have two possible formats:
// - 1 argument in the form of a JSON string (single address only). '[{"address":"<address>"}]'
// - 1 argument for a single address. &["<address>"]
pub(super) fn parse_max_send_value_args(args: &[&str]) -> Result<ZcashAddress, CommandError> {
    if args.len() != 1 {
        return Err(CommandError::InvalidArguments);
    }
    let address: ZcashAddress;

    if let Ok(addr) = address_from_str(args[0]) {
        address = addr;
    } else {
        let json_arg = json::parse(args[0]).map_err(|_e| CommandError::ArgNotJsonOrValidAddress)?;

        if json_arg.is_array() {
            return Err(CommandError::JsonArrayNotObj(
                "Pass an object, not an array.".to_string(),
            ));
        }
        if json_arg.is_empty() {
            return Err(CommandError::EmptyJsonArray);
        }
        address = address_from_json(&json_arg)?;
    }

    Ok(address)
}

// Checks send inputs do not contain memo's to transparent addresses.
fn check_memo_compatibility(
    address: &ZcashAddress,
    memo: &Option<MemoBytes>,
) -> Result<(), CommandError> {
    if !address.can_receive_memo() && memo.is_some() {
        return Err(CommandError::IncompatibleMemo);
    }

    Ok(())
}

fn address_from_json(json_array: &JsonValue) -> Result<ZcashAddress, CommandError> {
    if !json_array.has_key("address") {
        return Err(CommandError::MissingKey("address".to_string()));
    }
    let address_str = json_array["address"]
        .as_str()
        .ok_or(CommandError::UnexpectedType(
            "address is not a string!".to_string(),
        ))?;
    address_from_str(address_str).map_err(CommandError::ConversionFailed)
}

fn zatoshis_from_json(json_array: &JsonValue) -> Result<Zatoshis, CommandError> {
    if !json_array.has_key("amount") {
        return Err(CommandError::MissingKey("amount".to_string()));
    }
    let amount_u64 = if json_array["amount"].is_number() {
        json_array["amount"]
            .as_u64()
            .ok_or(CommandError::UnexpectedType(
                "amount not a u64!".to_string(),
            ))?
    } else {
        return Err(CommandError::NonJsonNumberForAmount(format!(
            "\"amount\": {}\nis not a json::number::Number",
            json_array["amount"]
        )));
    };
    zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)
}

fn memo_from_json(json_array: &JsonValue) -> Result<Option<MemoBytes>, CommandError> {
    if let Some(m) = json_array["memo"]
        .as_str()
        .map(std::string::ToString::to_string)
    {
        let memo = wallet::utils::memo_bytes_from_string(m).map_err(CommandError::InvalidMemo)?;
        Ok(Some(memo))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests;

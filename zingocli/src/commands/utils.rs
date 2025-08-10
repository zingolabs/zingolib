//! Module containing utility functions for the commands interface

use zcash_address::ZcashAddress;
use zcash_primitives::memo::MemoBytes;
use zcash_protocol::value::Zatoshis;

use crate::commands::error::CommandError;
use zingolib::data::receivers::Receivers;
use zingolib::utils::conversion::{address_from_str, zatoshis_from_u64};
use zingolib::wallet;

/// The possible arguments for `do_send`
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum SendArgs {
    /// Multiple sends in the form of a JSON array
    Multiple(Vec<SendArg>),

    /// Represents a single send
    Single(SendArg),
}

/// A send argument, used in `do_send`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SendArg {
    /// The address to send funds to
    pub address: String,

    /// The amount to send, in zatoshis. Can be specified as "value" or "amount"
    #[serde(alias = "amount", alias = "value")]
    pub value: u64,

    /// The optional memo to attach
    #[serde(default)]
    pub memo: Option<String>,
}

// Parse the send arguments for `do_send`.
// The send arguments have two possible formats:
// - 1 argument in the form of a JSON string for multiple sends. '[{"address":"<address>", "value":<value>, "memo":"<optional memo>"}, ...]'
// - 2 (+1 optional) arguments for a single address send. &["<address>", <amount>, "<optional memo>"]
pub(super) fn parse_send_args(args: &[&str]) -> Result<Receivers, CommandError> {
    match args {
        // JSON array form: '[{ "address": "...", "value": 123, "memo": "..."}, ...]'
        [json] => {
            let list: Vec<SendArg> =
                serde_json::from_str(json).map_err(CommandError::ArgsNotJson)?;
            if list.is_empty() {
                return Err(CommandError::EmptyJsonArray);
            }

            list.into_iter()
                .map(|sa| {
                    let recipient_address =
                        address_from_str(&sa.address).map_err(CommandError::ConversionFailed)?;
                    let amount =
                        zatoshis_from_u64(sa.value).map_err(CommandError::ConversionFailed)?;
                    let memo = match sa.memo {
                        Some(m) => Some(
                            wallet::utils::interpret_memo_string(m)
                                .map_err(CommandError::InvalidMemo)?,
                        ),
                        None => None,
                    };
                    check_memo_compatibility(&recipient_address, &memo)?;
                    Ok(zingolib::data::receivers::Receiver {
                        recipient_address,
                        amount,
                        memo,
                    })
                })
                .collect::<Result<Receivers, CommandError>>()
        }
        // Positional single-send: "<address>" "<amount>""
        [addr, amount] => {
            let recipient_address =
                address_from_str(addr).map_err(CommandError::ConversionFailed)?;
            let amount_u64 = amount
                .trim()
                .parse::<u64>()
                .map_err(CommandError::ParseIntFromString)?;
            let amount = zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)?;
            let memo = None;
            check_memo_compatibility(&recipient_address, &memo)?;
            Ok(vec![zingolib::data::receivers::Receiver {
                recipient_address,
                amount,
                memo,
            }])
        }
        // Positional single-send with memo: "<address>" "<amount>" "<memo>"
        [addr, amount, memo_str] => {
            let recipient_address =
                address_from_str(addr).map_err(CommandError::ConversionFailed)?;
            let amount_u64 = amount
                .trim()
                .parse::<u64>()
                .map_err(CommandError::ParseIntFromString)?;
            let amount = zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)?;
            let memo = Some(
                wallet::utils::interpret_memo_string((*memo_str).to_string())
                    .map_err(CommandError::InvalidMemo)?,
            );
            check_memo_compatibility(&recipient_address, &memo)?;
            Ok(vec![zingolib::data::receivers::Receiver {
                recipient_address,
                amount,
                memo,
            }])
        }
        _ => Err(CommandError::InvalidArguments),
    }
}

// The send arguments have two possible formats:
// - 1 arguments in the form of:
//    *  a JSON string (single address only). '[{"address":"<address>", "memo":"<optional memo>", "zennies_for_zingo":<true|false>}]'
// - 1 + 1 optional arguments for a single address send. &["<address>", "<optional memo>"]
pub(super) fn parse_send_all_args(
    args: &[&str],
) -> Result<(ZcashAddress, bool, Option<MemoBytes>), CommandError> {
    let address: ZcashAddress;
    let memo: Option<MemoBytes>;
    let zennies_for_zingo: bool;
    if args.len() == 1 {
        if let Ok(addr) = address_from_str(args[0]) {
            address = addr;
            memo = None;
            check_memo_compatibility(&address, &memo)?;
            zennies_for_zingo = false;
        } else {
            let json_arg = serde_json::from_str(args[0])
                .map_err(|_e| CommandError::ArgNotJsonOrValidAddress)?;
            if json_arg.is_array() {
                return Err(CommandError::JsonArrayNotObj(json_arg.to_string()));
            }
            if json_arg.is_empty() {
                return Err(CommandError::EmptyJsonArray);
            }
            address = address_from_json(&json_arg)?;
            memo = memo_from_json(&json_arg)?;
            check_memo_compatibility(&address, &memo)?;
            zennies_for_zingo = zennies_flag_from_json(&json_arg)?;
        }
    } else if args.len() == 2 {
        zennies_for_zingo = false;
        address = address_from_str(args[0]).map_err(CommandError::ConversionFailed)?;
        memo = Some(
            wallet::utils::interpret_memo_string(args[1].to_string())
                .map_err(CommandError::InvalidMemo)?,
        );
        check_memo_compatibility(&address, &memo)?;
    } else {
        return Err(CommandError::InvalidArguments);
    }
    Ok((address, zennies_for_zingo, memo))
}

// Parse the arguments for `spendable_balance`.
// The arguments have two possible formats:
// - 1 argument in the form of a JSON string (single address only). '[{"address":"<address>", "zennies_for_zingo": <true|false>}]'
// - 1 argument for a single address. &["<address>"]
// NOTE: zennies_for_zingo can only be set in a JSON
// string.
pub(super) fn parse_max_send_value_args(
    args: &[&str],
) -> Result<(ZcashAddress, bool), CommandError> {
    if args.len() > 2 {
        return Err(CommandError::InvalidArguments);
    }
    let address: ZcashAddress;
    let zennies_for_zingo: bool;

    if let Ok(addr) = address_from_str(args[0]) {
        address = addr;
        zennies_for_zingo = false;
    } else {
        let json_arg =
            serde_json::from_str(args[0]).map_err(|_e| CommandError::ArgNotJsonOrValidAddress)?;

        if json_arg.is_array() {
            return Err(CommandError::JsonArrayNotObj(
                "Pass an object, not an array.".to_string(),
            ));
        }
        if json_arg.is_empty() {
            return Err(CommandError::EmptyJsonArray);
        }
        address = address_from_json(&json_arg)?;
        zennies_for_zingo = zennies_flag_from_json(&json_arg)?;
    }

    Ok((address, zennies_for_zingo))
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

fn address_from_json(json_array: &serde_json::Value) -> Result<ZcashAddress, CommandError> {
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

fn zennies_flag_from_json(json_arg: &serde_json::Value) -> Result<bool, CommandError> {
    if !json_arg.has_key("zennies_for_zingo") {
        return Err(CommandError::MissingZenniesForZingoFlag);
    }
    match json_arg["zennies_for_zingo"].as_bool() {
        Some(boolean) => Ok(boolean),
        None => Err(CommandError::ZenniesFlagNonBool(
            json_arg["zennies_for_zingo"].to_string(),
        )),
    }
}

fn zatoshis_from_json(json_array: &serde_json::Value) -> Result<Zatoshis, CommandError> {
    if !json_array.has_key("amount") {
        return Err(CommandError::MissingKey("amount".to_string()));
    }
    let amount_u64 = if !json_array["amount"].is_number() {
        return Err(CommandError::NonJsonNumberForAmount(format!(
            "\"amount\": {}\nis not a number",
            json_array["amount"]
        )));
    } else {
        json_array["amount"]
            .as_u64()
            .ok_or(CommandError::UnexpectedType(
                "amount not a u64!".to_string(),
            ))?
    };
    zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)
}

fn memo_from_json(json_array: &serde_json::Value) -> Result<Option<MemoBytes>, CommandError> {
    if let Some(m) = json_array["memo"].as_str().map(|s| s.to_string()) {
        let memo = wallet::utils::interpret_memo_string(m).map_err(CommandError::InvalidMemo)?;
        Ok(Some(memo))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use zingolib::{
        data::receivers::Receiver,
        utils::conversion::{address_from_str, zatoshis_from_u64},
        wallet::{self, utils::interpret_memo_string},
    };

    use crate::commands::error::CommandError;

    #[test]
    fn parse_send_args() {
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let recipient_address = address_from_str(address_str).unwrap();
        let value_str = "100000";
        let amount = zatoshis_from_u64(100_000).unwrap();
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // No memo
        let send_args = &[address_str, value_str];
        assert_eq!(
            super::parse_send_args(send_args).unwrap(),
            vec![zingolib::data::receivers::Receiver {
                recipient_address: recipient_address.clone(),
                amount,
                memo: None
            }]
        );

        // Memo
        let send_args = &[address_str, value_str, memo_str];
        assert_eq!(
            super::parse_send_args(send_args).unwrap(),
            vec![Receiver {
                recipient_address: recipient_address.clone(),
                amount,
                memo: Some(memo.clone())
            }]
        );

        // Json
        let json = "[{\"address\":\"tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd\", \"amount\":50000}, \
                    {\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        assert_eq!(
            super::parse_send_args(&[json]).unwrap(),
            vec![
                Receiver {
                    recipient_address: address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd")
                        .unwrap(),
                    amount: zatoshis_from_u64(50_000).unwrap(),
                    memo: None
                },
                Receiver {
                    recipient_address: recipient_address.clone(),
                    amount,
                    memo: Some(memo.clone())
                }
            ]
        );

        // Trim whitespace
        let send_args = &[address_str, "1 ", memo_str];
        assert_eq!(
            super::parse_send_args(send_args).unwrap(),
            vec![Receiver {
                recipient_address,
                amount: zatoshis_from_u64(1).unwrap(),
                memo: Some(memo.clone())
            }]
        );
    }

    mod fail_parse_send_args {
        use crate::commands::{error::CommandError, utils::parse_send_args};

        mod json_array {
            use super::*;

            #[test]
            fn empty_json_array() {
                let json = "[]";
                assert!(matches!(
                    parse_send_args(&[json]),
                    Err(CommandError::EmptyJsonArray)
                ));
            }
            #[test]
            fn failed_json_parsing() {
                let args = [r#"testaddress{{"#];
                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::ArgsNotJson(_))
                ));
            }
            #[test]
            fn single_arg_not_an_array_unexpected_type() {
                let args = ["1"];
                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::SingleArgNotJsonArray(_))
                ));
            }
            #[test]
            fn invalid_memo() {
                let arg_contents = "[{\"address\": \"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \"amount\": 123, \"memo\": \"testmemo\"}]";
                let long_513_byte_memo = &"a".repeat(513);
                let long_memo_args =
                    arg_contents.replace("\"testmemo\"", &format!("\"{}\"", long_513_byte_memo));
                let args = [long_memo_args.as_str()];

                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::InvalidMemo(_))
                ));
            }
        }
        mod multi_string_args {
            use super::*;

            #[test]
            fn two_args_wrong_amount() {
                let args = [
                    "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                    "foo",
                ];
                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::ParseIntFromString(_))
                ));
            }
            #[test]
            fn wrong_number_of_args() {
                let args = [
                    "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                    "123",
                    "3",
                    "4",
                ];
                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::InvalidArguments)
                ));
            }
            #[test]
            fn invalid_memo() {
                let long_513_byte_memo = &"a".repeat(513);
                let args = [
                    "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                    "123",
                    long_513_byte_memo,
                ];

                assert!(matches!(
                    parse_send_args(&args),
                    Err(CommandError::InvalidMemo(_))
                ));
            }
        }
    }

    #[test]
    fn parse_send_all_args() {
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let address = address_from_str(address_str).unwrap();
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // JSON single receiver
        let single_receiver = &[
            "{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                 \"memo\":\"test memo\", \
                 \"zennies_for_zingo\":false}",
        ];
        assert_eq!(
            super::parse_send_all_args(single_receiver).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );
        // NonBool Zenny Flag
        let nb_zenny = &[
            "{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                 \"memo\":\"test memo\", \
                 \"zennies_for_zingo\":\"false\"}",
        ];
        assert!(matches!(
            super::parse_send_all_args(nb_zenny),
            Err(CommandError::ZenniesFlagNonBool(_))
        ));
        // with memo
        let send_args = &[address_str, memo_str];
        assert_eq!(
            super::parse_send_all_args(send_args).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );
        let send_args = &[address_str, memo_str];
        assert_eq!(
            super::parse_send_all_args(send_args).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );

        // invalid address
        let send_args = &["invalid_address"];
        assert!(matches!(
            super::parse_send_all_args(send_args),
            Err(CommandError::ArgNotJsonOrValidAddress)
        ));
    }

    #[test]
    fn check_memo_compatibility() {
        let sapling_address = address_from_str("zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p").unwrap();
        let transparent_address = address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd").unwrap();
        let memo = interpret_memo_string("test memo".to_string()).unwrap();

        // shielded address with memo
        super::check_memo_compatibility(&sapling_address, &Some(memo.clone())).unwrap();

        // transparent address without memo
        super::check_memo_compatibility(&transparent_address, &None).unwrap();

        // transparent address with memo
        assert!(matches!(
            super::check_memo_compatibility(&transparent_address, &Some(memo.clone())),
            Err(CommandError::IncompatibleMemo)
        ));
    }

    #[test]
    fn address_from_json() {
        // with address
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        super::address_from_json(json_args).unwrap();

        // without address
        let json_str = "[{\"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::address_from_json(json_args),
            Err(CommandError::MissingKey(_))
        ));

        // invalid address
        let json_str = "[{\"address\": 1, \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::address_from_json(json_args),
            Err(CommandError::UnexpectedType(_))
        ));
    }

    #[test]
    fn zatoshis_from_json() {
        // with amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        super::zatoshis_from_json(json_args).unwrap();

        // without amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::zatoshis_from_json(json_args),
            Err(CommandError::MissingKey(_))
        ));

        // invalid amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":\"non_number\", \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::zatoshis_from_json(json_args),
            Err(CommandError::NonJsonNumberForAmount(_))
        ));
    }

    #[test]
    fn memo_from_json() {
        // with memo
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert_eq!(
            super::memo_from_json(json_args).unwrap(),
            Some(interpret_memo_string("test memo".to_string()).unwrap())
        );

        // without memo
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000}]";
        let json_args = serde_json::from_str(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert_eq!(super::memo_from_json(json_args).unwrap(), None);
    }
}

// Module containing utility functions for the commands interface

use crate::commands::error::CommandError;
use crate::config::ChainType;
use crate::data::receivers::Receivers;
use crate::utils::conversion::{address_from_str, zatoshis_from_u64};
use crate::wallet;
use json::JsonValue;
use zcash_client_backend::address::Address;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;

// Parse the send arguments for `do_send`.
// The send arguments have two possible formats:
// - 1 argument in the form of a JSON string for multiple sends. '[{"address":"<address>", "value":<value>, "memo":"<optional memo>"}, ...]'
// - 2 (+1 optional) arguments for a single address send. &["<address>", <amount>, "<optional memo>"]
pub(super) fn parse_send_args(args: &[&str], chain: &ChainType) -> Result<Receivers, CommandError> {
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
                let recipient_address = address_from_json(j, chain)?;
                let amount = zatoshis_from_json(j)?;
                let memo = memo_from_json(j)?;
                check_memo_compatibility(&recipient_address, &memo)?;

                Ok(crate::data::receivers::Receiver {
                    recipient_address,
                    amount,
                    memo,
                })
            })
            .collect::<Result<Receivers, CommandError>>()
    } else if args.len() == 2 || args.len() == 3 {
        let recipient_address =
            address_from_str(args[0], chain).map_err(CommandError::ConversionFailed)?;
        let amount_u64 = args[1]
            .trim()
            .parse::<u64>()
            .map_err(CommandError::ParseIntFromString)?;
        let amount = zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)?;
        let memo = if args.len() == 3 {
            Some(
                wallet::utils::interpret_memo_string(args[2].to_string())
                    .map_err(CommandError::InvalidMemo)?,
            )
        } else {
            None
        };
        check_memo_compatibility(&recipient_address, &memo)?;

        Ok(vec![crate::data::receivers::Receiver {
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
//    *  a JSON string (single address only). '[{"address":"<address>", "memo":"<optional memo>", "zennies_for_zingo":<true|false>}]'
// - 1 + 1 optional arguments for a single address send. &["<address>", "<optional memo>"]
pub(super) fn parse_send_all_args(
    args: &[&str],
    chain: &ChainType,
) -> Result<(Address, bool, Option<MemoBytes>), CommandError> {
    let address: Address;
    let memo: Option<MemoBytes>;
    let zennies_for_zingo: bool;
    if args.len() == 1 {
        if let Ok(addr) = address_from_str(args[0], chain) {
            address = addr;
            memo = None;
            check_memo_compatibility(&address, &memo)?;
            zennies_for_zingo = false;
        } else {
            let json_arg =
                json::parse(args[0]).map_err(|_e| CommandError::ArgNotJsonOrValidAddress)?;
            if json_arg.is_array() {
                return Err(CommandError::JsonArrayNotObj(json_arg.to_string()));
            }
            if json_arg.is_empty() {
                return Err(CommandError::EmptyJsonArray);
            }
            address = address_from_json(&json_arg, chain)?;
            memo = memo_from_json(&json_arg)?;
            check_memo_compatibility(&address, &memo)?;
            zennies_for_zingo = zennies_flag_from_json(&json_arg)?;
        }
    } else if args.len() == 2 {
        zennies_for_zingo = false;
        address = address_from_str(args[0], chain).map_err(CommandError::ConversionFailed)?;
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
pub(super) fn parse_spendable_balance_args(
    args: &[&str],
    chain: &ChainType,
) -> Result<(Address, bool), CommandError> {
    if args.len() > 2 {
        return Err(CommandError::InvalidArguments);
    }
    let address: Address;
    let zennies_for_zingo: bool;

    if let Ok(addr) = address_from_str(args[0], chain) {
        address = addr;
        zennies_for_zingo = false;
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
        address = address_from_json(&json_arg, chain)?;
        zennies_for_zingo = zennies_flag_from_json(&json_arg)?;
    }

    Ok((address, zennies_for_zingo))
}

// Checks send inputs do not contain memo's to transparent addresses.
fn check_memo_compatibility(
    address: &Address,
    memo: &Option<MemoBytes>,
) -> Result<(), CommandError> {
    if let Address::Transparent(_) = address {
        if memo.is_some() {
            return Err(CommandError::IncompatibleMemo);
        }
    }

    Ok(())
}

fn address_from_json(json_array: &JsonValue, chain: &ChainType) -> Result<Address, CommandError> {
    if !json_array.has_key("address") {
        return Err(CommandError::MissingKey("address".to_string()));
    }
    let address_str = json_array["address"]
        .as_str()
        .ok_or(CommandError::UnexpectedType(
            "address is not a string!".to_string(),
        ))?;
    address_from_str(address_str, chain).map_err(CommandError::ConversionFailed)
}

fn zennies_flag_from_json(json_arg: &JsonValue) -> Result<bool, CommandError> {
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
fn zatoshis_from_json(json_array: &JsonValue) -> Result<NonNegativeAmount, CommandError> {
    if !json_array.has_key("amount") {
        return Err(CommandError::MissingKey("amount".to_string()));
    }
    let amount_u64 = if !json_array["amount"].is_number() {
        return Err(CommandError::NonJsonNumberForAmount(format!(
            "\"amount\": {}\nis not a json::number::Number",
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

fn memo_from_json(json_array: &JsonValue) -> Result<Option<MemoBytes>, CommandError> {
    if let Some(m) = json_array["memo"].as_str().map(|s| s.to_string()) {
        let memo = wallet::utils::interpret_memo_string(m).map_err(CommandError::InvalidMemo)?;
        Ok(Some(memo))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{ChainType, RegtestNetwork};

    use crate::{
        commands::error::CommandError,
        data::receivers::Receiver,
        utils::conversion::{address_from_str, zatoshis_from_u64},
        wallet::{self, utils::interpret_memo_string},
    };

    #[test]
    fn parse_send_args() {
        let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let recipient_address = address_from_str(address_str, &chain).unwrap();
        let value_str = "100000";
        let amount = zatoshis_from_u64(100_000).unwrap();
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // No memo
        let send_args = &[address_str, value_str];
        assert_eq!(
            super::parse_send_args(send_args, &chain).unwrap(),
            vec![crate::data::receivers::Receiver {
                recipient_address: recipient_address.clone(),
                amount,
                memo: None
            }]
        );

        // Memo
        let send_args = &[address_str, value_str, memo_str];
        assert_eq!(
            super::parse_send_args(send_args, &chain).unwrap(),
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
            super::parse_send_args(&[json], &chain).unwrap(),
            vec![
                Receiver {
                    recipient_address: address_from_str(
                        "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
                        &chain
                    )
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
            super::parse_send_args(send_args, &chain).unwrap(),
            vec![Receiver {
                recipient_address,
                amount: zatoshis_from_u64(1).unwrap(),
                memo: Some(memo.clone())
            }]
        );
    }

    mod fail_parse_send_args {
        use crate::config::{ChainType, RegtestNetwork};

        use crate::commands::{error::CommandError, utils::parse_send_args};

        mod json_array {
            use super::*;

            #[test]
            fn empty_json_array() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let json = "[]";
                assert!(matches!(
                    parse_send_args(&[json], &chain),
                    Err(CommandError::EmptyJsonArray)
                ));
            }
            #[test]
            fn failed_json_parsing() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let args = [r#"testaddress{{"#];
                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::ArgsNotJson(_))
                ));
            }
            #[test]
            fn single_arg_not_an_array_unexpected_type() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let args = ["1"];
                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::SingleArgNotJsonArray(_))
                ));
            }
            #[test]
            fn invalid_memo() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let arg_contents =
                    "[{\"address\": \"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \"amount\": 123, \"memo\": \"testmemo\"}]";
                let long_513_byte_memo = &"a".repeat(513);
                let long_memo_args =
                    arg_contents.replace("\"testmemo\"", &format!("\"{}\"", long_513_byte_memo));
                let args = [long_memo_args.as_str()];

                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::InvalidMemo(_))
                ));
            }
        }
        mod multi_string_args {
            use super::*;

            #[test]
            fn two_args_wrong_amount() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "foo"];
                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::ParseIntFromString(_))
                ));
            }
            #[test]
            fn wrong_number_of_args() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "123", "3", "4"];
                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::InvalidArguments)
                ));
            }
            #[test]
            fn invalid_memo() {
                let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
                let long_513_byte_memo = &"a".repeat(513);
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "123", long_513_byte_memo];

                assert!(matches!(
                    parse_send_args(&args, &chain),
                    Err(CommandError::InvalidMemo(_))
                ));
            }
        }
    }

    #[test]
    fn parse_send_all_args() {
        let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let address = address_from_str(address_str, &chain).unwrap();
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // JSON single receiver
        let single_receiver =
            &["{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                 \"memo\":\"test memo\", \
                 \"zennies_for_zingo\":false}"];
        assert_eq!(
            super::parse_send_all_args(single_receiver, &chain).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );
        // NonBool Zenny Flag
        let nb_zenny =
            &["{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                 \"memo\":\"test memo\", \
                 \"zennies_for_zingo\":\"false\"}"];
        assert!(matches!(
            super::parse_send_all_args(nb_zenny, &chain),
            Err(CommandError::ZenniesFlagNonBool(_))
        ));
        // with memo
        let send_args = &[address_str, memo_str];
        assert_eq!(
            super::parse_send_all_args(send_args, &chain).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );
        let send_args = &[address_str, memo_str];
        assert_eq!(
            super::parse_send_all_args(send_args, &chain).unwrap(),
            (address.clone(), false, Some(memo.clone()))
        );

        // invalid address
        let send_args = &["invalid_address"];
        assert!(matches!(
            super::parse_send_all_args(send_args, &chain),
            Err(CommandError::ArgNotJsonOrValidAddress)
        ));
    }

    #[test]
    fn check_memo_compatibility() {
        let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());
        let sapling_address = address_from_str("zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", &chain).unwrap();
        let transparent_address =
            address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd", &chain).unwrap();
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
        let chain = ChainType::Regtest(RegtestNetwork::all_upgrades_active());

        // with address
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        super::address_from_json(json_args, &chain).unwrap();

        // without address
        let json_str = "[{\"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::address_from_json(json_args, &chain),
            Err(CommandError::MissingKey(_))
        ));

        // invalid address
        let json_str = "[{\"address\": 1, \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::address_from_json(json_args, &chain),
            Err(CommandError::UnexpectedType(_))
        ));
    }

    #[test]
    fn zatoshis_from_json() {
        // with amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        super::zatoshis_from_json(json_args).unwrap();

        // without amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert!(matches!(
            super::zatoshis_from_json(json_args),
            Err(CommandError::MissingKey(_))
        ));

        // invalid amount
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":\"non_number\", \"memo\":\"test memo\"}]";
        let json_args = json::parse(json_str).unwrap();
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
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert_eq!(
            super::memo_from_json(json_args).unwrap(),
            Some(interpret_memo_string("test memo".to_string()).unwrap())
        );

        // without memo
        let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000}]";
        let json_args = json::parse(json_str).unwrap();
        let json_args = json_args.members().next().unwrap();
        assert_eq!(super::memo_from_json(json_args).unwrap(), None);
    }
}

// Module containing utility functions for the commands interface

use crate::commands::error::CommandError;
use crate::utils::{address_from_str, zatoshis_from_u64};
use crate::wallet::{self, Pool};
use zcash_client_backend::address::Address;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zingoconfig::ChainType;

pub(super) fn parse_shield_args(
    args: &[&str],
    chain: &ChainType,
) -> Result<(Vec<Pool>, Option<Address>), CommandError> {
    if args.is_empty() || args.len() > 2 {
        return Err(CommandError::InvalidArguments);
    }

    let pools_to_shield: &[Pool] = match args[0] {
        "transparent" => &[Pool::Transparent],
        "sapling" => &[Pool::Sapling],
        "all" => &[Pool::Sapling, Pool::Transparent],
        _ => return Err(CommandError::InvalidPool),
    };
    let address = if args.len() == 2 {
        Some(address_from_str(args[1], chain).map_err(CommandError::ConversionFailed)?)
    } else {
        None
    };

    Ok((pools_to_shield.to_vec(), address))
}

// Parse the send arguments for `do_propose`.
// The send arguments have two possible formats:
// - 1 argument in the form of a JSON string for multiple sends. '[{"address":"<address>", "value":<value>, "memo":"<optional memo>"}, ...]'
// - 2 (+1 optional) arguments for a single address send. &["<address>", <amount>, "<optional memo>"]
pub(super) fn parse_send_args(
    args: &[&str],
    chain: &ChainType,
) -> Result<Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>, CommandError> {
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
                if !j.has_key("address") {
                    return Err(CommandError::MissingKey("address".to_string()));
                }
                let address_str = j["address"].as_str().ok_or(CommandError::UnexpectedType(
                    "address not a Str!".to_string(),
                ))?;
                let address =
                    address_from_str(address_str, chain).map_err(CommandError::ConversionFailed)?;

                if !j.has_key("amount") {
                    return Err(CommandError::MissingKey("amount".to_string()));
                }
                let amount_u64 = if !j["amount"].is_number() {
                    return Err(CommandError::NonJsonNumberForAmount(format!(
                        "\"amount\": {}\nis not a json::number::Number",
                        j["amount"]
                    )));
                } else {
                    j["amount"].as_u64().ok_or(CommandError::UnexpectedType(
                        "amount not a u64!".to_string(),
                    ))?
                };
                let amount =
                    zatoshis_from_u64(amount_u64).map_err(CommandError::ConversionFailed)?;

                let memo = if let Some(m) = j["memo"].as_str().map(|s| s.to_string()) {
                    Some(
                        wallet::utils::interpret_memo_string(m)
                            .map_err(CommandError::InvalidMemo)?,
                    )
                } else {
                    None
                };
                check_memo_compatibility(&address, &memo)?;

                Ok((address, amount, memo))
            })
            .collect::<Result<Vec<(Address, NonNegativeAmount, Option<MemoBytes>)>, CommandError>>()
    } else if args.len() == 2 || args.len() == 3 {
        let address = address_from_str(args[0], chain).map_err(CommandError::ConversionFailed)?;
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
        check_memo_compatibility(&address, &memo)?;

        Ok(vec![(address, amount, memo)])
    } else {
        return Err(CommandError::InvalidArguments);
    }?;

    Ok(send_args)
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

#[cfg(test)]
mod tests {
    use zingoconfig::{ChainType, RegtestNetwork};

    use crate::{
        commands::error::CommandError,
        utils::{address_from_str, zatoshis_from_u64},
        wallet::{self, utils::interpret_memo_string, Pool},
    };

    #[test]
    fn parse_shield_args() {
        let chain = ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let address = address_from_str(address_str, &chain).unwrap();

        // Shield all to default address
        let shield_args = &["all"];
        assert_eq!(
            super::parse_shield_args(shield_args, &chain).unwrap(),
            (vec![Pool::Sapling, Pool::Transparent], None)
        );

        // Shield all to given address
        let shield_args = &["all", address_str];
        assert_eq!(
            super::parse_shield_args(shield_args, &chain).unwrap(),
            (vec![Pool::Sapling, Pool::Transparent], Some(address))
        );

        // Invalid pool
        let shield_args = &["invalid"];
        assert!(matches!(
            super::parse_shield_args(shield_args, &chain),
            Err(CommandError::InvalidPool)
        ));
    }

    #[test]
    fn parse_send_args() {
        let chain = ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
        let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let address = address_from_str(address_str, &chain).unwrap();
        let value_str = "100000";
        let value = zatoshis_from_u64(100_000).unwrap();
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // No memo
        let send_args = &[address_str, value_str];
        assert_eq!(
            super::parse_send_args(send_args, &chain).unwrap(),
            vec![(address.clone(), value, None)]
        );

        // Memo
        let send_args = &[address_str, value_str, memo_str];
        assert_eq!(
            super::parse_send_args(send_args, &chain).unwrap(),
            vec![(address.clone(), value, Some(memo.clone()))]
        );

        // Json
        let json = "[{\"address\":\"tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd\", \"amount\":50000}, \
                    {\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
        assert_eq!(
            super::parse_send_args(&[json], &chain).unwrap(),
            vec![
                (
                    address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd", &chain).unwrap(),
                    zatoshis_from_u64(50_000).unwrap(),
                    None
                ),
                (address.clone(), value, Some(memo.clone()))
            ]
        );

        // Trim whitespace
        let send_args = &[address_str, "1 ", memo_str];
        assert_eq!(
            super::parse_send_args(send_args, &chain).unwrap(),
            vec![(address, zatoshis_from_u64(1).unwrap(), Some(memo.clone()))]
        );
    }

    mod fail_parse_send_args {
        use zingoconfig::{ChainType, RegtestNetwork};

        use crate::commands::{error::CommandError, utils::parse_send_args};
        mod json_array {
            use super::*;

            #[test]
            fn empty_json_array() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let json = "[]";
                assert!(matches!(
                    parse_send_args(&[json], &chain),
                    Err(CommandError::EmptyJsonArray)
                ));
            }
            #[test]
            fn failed_json_parsing() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = [r#"testaddress{{"#];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::ArgsNotJson(e)) => match e {
                        json::Error::UnexpectedCharacter { ch, line, column } => {
                            assert_eq!(ch, 'e');
                            assert_eq!(line, 1);
                            assert_eq!(column, 2);
                        }
                        _ => panic!(),
                    },
                    _ => panic!(),
                };
            }
            #[test]
            fn single_arg_not_an_array_unexpected_type() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["1"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::SingleArgNotJsonArray(e)) => assert_eq!(e, "1".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn no_address_missing_key() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["[{\"amount\": 123, \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::MissingKey(e)) => assert_eq!(e, "address".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn no_amount_missing_key() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["[{\"address\": \"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::MissingKey(e)) => assert_eq!(e, "amount".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn non_string_address() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["[{\"address\": 1, \"amount\": 123, \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::UnexpectedType(e)) => {
                        assert_eq!(e, "address not a Str!".to_string())
                    }
                    _ => panic!(),
                };
            }
            #[test]
            fn non_u64_amount() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args =
                            ["[{\"address\": \"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \"amount\": \"Oscar Pepper\", \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::NonJsonNumberForAmount(e)) => {
                        assert_eq!(
                            e,
                            "\"amount\": Oscar Pepper\nis not a json::number::Number".to_string()
                        )
                    }
                    _ => panic!(),
                };
            }
            #[test]
            fn invalid_memo() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let arg_contents =
                    "[{\"address\": \"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \"amount\": 123, \"memo\": \"testmemo\"}]";
                let long_513_byte_memo = &"a".repeat(513);
                let long_memo_args =
                    arg_contents.replace("\"testmemo\"", &format!("\"{}\"", long_513_byte_memo));
                let args = [long_memo_args.as_str()];

                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::InvalidMemo(e)) => {
                        assert_eq!(
                            e,
                            format!(
                                "Error creating output. Memo '\"{}\"' is too long",
                                long_513_byte_memo
                            )
                        )
                    }
                    _ => panic!(),
                };
            }
        }
        mod multi_string_args {
            use super::*;

            #[test]
            fn two_args_wrong_amount() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "foo"];
                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::ParseIntFromString(e)) => {
                        assert_eq!(
                            "invalid digit found in string".to_string(),
                            format!("{}", e)
                        )
                    }
                    _ => panic!(),
                };
            }
            #[test]
            fn wrong_number_of_args() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "123", "3", "4"];
                let result = parse_send_args(&args, &chain);
                assert!(matches!(result, Err(CommandError::InvalidArguments)));
            }
            #[test]
            fn invalid_memo() {
                let chain =
                    ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
                let long_513_byte_memo = &"a".repeat(513);
                let args = ["zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", "123", long_513_byte_memo];

                let result = parse_send_args(&args, &chain);
                match result {
                    Err(CommandError::InvalidMemo(e)) => {
                        assert_eq!(
                            e,
                            format!(
                                "Error creating output. Memo '\"{}\"' is too long",
                                long_513_byte_memo
                            )
                        )
                    }
                    _ => panic!(),
                };
            }
        }
    }

    #[test]
    fn check_memo_compatibility() {
        let chain = ChainType::Regtest(RegtestNetwork::set_all_net_upgrades_to_active_at_1());
        let sapling_address = address_from_str("zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p", &chain).unwrap();
        let transparent_address =
            address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd", &chain).unwrap();
        let memo = interpret_memo_string("test memo".to_string()).unwrap();

        // shielded address with memo
        super::check_memo_compatibility(&sapling_address, &Some(memo.clone())).unwrap();

        // transparent address without memo
        super::check_memo_compatibility(&transparent_address, &None).unwrap();

        // transparent address with memo
        match super::check_memo_compatibility(&transparent_address, &Some(memo.clone())) {
            Err(CommandError::IncompatibleMemo) => (),
            _ => panic!(),
        };
    }
}

//! Module containing utility functions for the commands interface

use crate::commands::error::CommandError;
use crate::wallet;
use zcash_primitives::memo::MemoBytes;

/// Send args accepts two different formats for its input
pub(super) fn parse_send_args(
    args: &[&str],
) -> Result<Vec<(String, u64, Option<MemoBytes>)>, CommandError> {
    // Check for a single argument that can be parsed as JSON
    let send_args = if args.len() == 1 {
        let json_args = json::parse(args[0]).map_err(CommandError::ArgsNotJson)?;

        if !json_args.is_array() {
            return Err(CommandError::SingleArgNotJsonArray(json_args.to_string()));
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
                    .ok_or(CommandError::UnexpectedType(
                        "address not a Str!".to_string(),
                    ))?
                    .to_string();
                let amount = if !j["amount"].is_number() {
                    return Err(CommandError::NonJsonNumberForAmount(format!(
                        "\"amount\": {}\nis not a json::number::Number",
                        j["amount"]
                    )));
                } else {
                    j["amount"].as_u64().ok_or(CommandError::UnexpectedType(
                        "amount not a u64!".to_string(),
                    ))?
                };
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
            .trim()
            .parse::<u64>()
            .map_err(CommandError::ParseIntFromString)?;
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

#[cfg(test)]
mod tests {
    use crate::wallet;

    #[test]
    fn parse_send_args_test() {
        let address = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let value_str = "100000";
        let value = 100_000;
        let memo_str = "test memo";
        let memo = wallet::utils::interpret_memo_string(memo_str.to_string()).unwrap();

        // No memo
        let send_args = &[address, value_str];
        assert_eq!(
            super::parse_send_args(send_args).unwrap(),
            vec![(address.to_string(), value, None)]
        );

        // Memo
        let send_args = &[address, value_str, memo_str];
        assert_eq!(
            super::parse_send_args(send_args).unwrap(),
            vec![(address.to_string(), value, Some(memo.clone()))]
        );

        // Json
        let json = "[{\"address\":\"tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd\", \"amount\":50000}, \
            {\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
            \"amount\":100000, \"memo\":\"test memo\"}]";
        assert_eq!(
            super::parse_send_args(&[json]).unwrap(),
            vec![
                (
                    "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd".to_string(),
                    50_000,
                    None
                ),
                (address.to_string(), value, Some(memo))
            ]
        );
    }

    mod fail_parse_send_args {
        use zcash_primitives::memo::MemoBytes;

        use crate::commands::utils::parse_send_args;

        mod json_array {
            use crate::commands::{error::CommandError, utils::parse_send_args};
            #[test]
            fn failed_json_parsing() {
                let args = [r#"testaddress{{"#];
                let result = parse_send_args(&args);
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
                let args = ["1"];
                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::SingleArgNotJsonArray(e)) => assert_eq!(e, "1".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn no_address_missing_key() {
                let args = ["[{\"amount\": 123, \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::MissingKey(e)) => assert_eq!(e, "address".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn no_amount_missing_key() {
                let args = ["[{\"address\": \"testaddress\", \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::MissingKey(e)) => assert_eq!(e, "amount".to_string()),
                    _ => panic!(),
                };
            }
            #[test]
            fn non_string_address() {
                let args = ["[{\"address\": 1, \"amount\": 123, \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::UnexpectedType(e)) => {
                        assert_eq!(e, "address not a Str!".to_string())
                    }
                    _ => panic!(),
                };
            }
            #[test]
            fn non_u64_amount() {
                let args =
                ["[{\"address\": \"testaddress\", \"amount\": \"Oscar Pepper\", \"memo\": \"testmemo\"}]"];
                let result = parse_send_args(&args);
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
                let arg_contents =
                    "[{\"address\": \"testaddress\", \"amount\": 123, \"memo\": \"testmemo\"}]";
                let long_513_byte_memo = &"a".repeat(513);
                let long_memo_args =
                    arg_contents.replace("\"testmemo\"", &format!("\"{}\"", long_513_byte_memo));
                let args = [long_memo_args.as_str()];

                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::InvalidMemo(e)) => {
                        assert_eq!(
                            e,
                            format!(
                                "Error creating output. Memo '\"{}\"' is too long",
                                long_513_byte_memo.to_string()
                            )
                        )
                    }
                    _ => panic!(),
                };
            }
        }
        mod multi_string_args {
            use crate::commands::{error::CommandError, utils::parse_send_args};
            #[test]
            fn two_args_wrong_amount() {
                let args = ["testaddress", "foo"];
                let result = parse_send_args(&args);
                dbg!(&result);
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
            fn three_args_wrong_amount_show_trim_requirement() {
                // Note the " " character after the 1.  The parser can handle by trimming, is that correct?
                let args = ["testaddress", "1 ", "whatever"];
                let result = parse_send_args(&args);
                dbg!(&result);
                match result {
                    Ok(_) => (),
                    _ => panic!(),
                };
            }
            #[test]
            fn wrong_number_of_args() {
                let args = ["testaddress", "123", "3", "4"];
                let result = parse_send_args(&args);
                dbg!(&result);
                assert!(matches!(result, Err(CommandError::InvalidArguments)));
            }
            #[test]
            fn invalid_memo() {
                let long_513_byte_memo = &"a".repeat(513);
                let args = ["testaddress", "123", long_513_byte_memo];

                let result = parse_send_args(&args);
                match result {
                    Err(CommandError::InvalidMemo(e)) => {
                        assert_eq!(
                            e,
                            format!(
                                "Error creating output. Memo '\"{}\"' is too long",
                                long_513_byte_memo.to_string()
                            )
                        )
                    }
                    _ => panic!(),
                };
            }
        }

        #[test]
        fn successful_parse_send_args_single_json() {
            let args =
                ["[{\"address\": \"testaddress\", \"amount\": 123, \"memo\": \"testmemo\"}]"];
            let parsed = parse_send_args(&args).unwrap();
            // Assuming you have a way to construct MemoBytes from a string for this example
            assert_eq!(
                parsed,
                vec![(
                    "testaddress".to_string(),
                    123,
                    Some(MemoBytes::from_bytes(&"testmemo".as_bytes()).unwrap())
                )]
            );
        }
    }
}

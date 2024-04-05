#![forbid(unsafe_code)]
//! Currently this mod contains a utility fn that's been factored out of SendCommand

use crate::commands::error::CommandError;
use crate::wallet;
use zcash_primitives::memo::MemoBytes;

/// Send args accepts two different formats for its input
pub(super) fn parse_send_args(
    args: &[&str],
) -> Result<Vec<(String, u64, Option<MemoBytes>)>, CommandError> {
    // Check for a single argument that can be parsed as JSON
    let send_args = if args.len() == 1 {
        let json_args = json::parse(args[0]).map_err(CommandError::FailedJsonParsing)?;

        if !json_args.is_array() {
            return Err(CommandError::UnexpectedType(json_args.to_string()));
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
                let amount = j["amount"].as_u64().ok_or(CommandError::UnexpectedType(
                    "amount not a u64!".to_string(),
                ))?;
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
        dbg!(&address);
        let amount = args[1]
            .parse::<u64>()
            .map_err(CommandError::FailedIntParsing)?;
        dbg!(&amount);
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
    fn parse_send_args() {
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
}
#[cfg(test)]
mod tests {
    use super::*;
    mod failures {
        use super::*;
        #[test]
        fn parse_send_args_failed_json_parsing() {
            let args = [r#"testaddress{{"#];
            let result = parse_send_args(&args);
            match result {
                Err(CommandError::FailedJsonParsing(e)) => match e {
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
        fn parse_send_args_unexpected_type() {
            let args = ["1"];
            let result = parse_send_args(&args);
            match result {
                Err(CommandError::UnexpectedType(e)) => assert_eq!(e, "1".to_string()),
                _ => panic!(),
            };
        }

        #[test]
        fn parse_send_args_failure() {
            let args = ["testaddress", "123", "3", "4"];
            let result = parse_send_args(&args);
            dbg!(&result);
            assert!(matches!(result, Err(CommandError::InvalidArguments)));
        }
    }

    #[test]
    fn successful_parse_send_args_single_json() {
        let args = ["[{\"address\": \"testaddress\", \"amount\": 123, \"memo\": \"testmemo\"}]"];
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

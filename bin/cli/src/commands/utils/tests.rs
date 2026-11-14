use zingolib::{
    data::receivers::Receiver,
    utils::conversion::{address_from_str, zatoshis_from_u64},
    wallet::{self, utils::memo_bytes_from_string},
};

use crate::commands::error::CommandError;

#[test]
fn parse_send_args() {
    let address_str = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
    let recipient_address = address_from_str(address_str).unwrap();
    let value_str = "100000";
    let amount = zatoshis_from_u64(100_000).unwrap();
    let memo_str = "test memo";
    let memo = wallet::utils::memo_bytes_from_string(memo_str.to_string()).unwrap();

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
                recipient_address: address_from_str("tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd").unwrap(),
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
            let args = [r"testaddress{{"];
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
                arg_contents.replace("\"testmemo\"", &format!("\"{long_513_byte_memo}\""));
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
    let memo = wallet::utils::memo_bytes_from_string(memo_str.to_string()).unwrap();

    // JSON single receiver
    let single_receiver = &[
        "{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                 \"memo\":\"test memo\"}",
    ];
    assert_eq!(
        super::parse_send_all_args(single_receiver).unwrap(),
        (address.clone(), Some(memo.clone()))
    );
    // with memo
    let send_args = &[address_str, memo_str];
    assert_eq!(
        super::parse_send_all_args(send_args).unwrap(),
        (address.clone(), Some(memo.clone()))
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
    let memo = memo_bytes_from_string("test memo".to_string()).unwrap();

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
    let json_args = json::parse(json_str).unwrap();
    let json_args = json_args.members().next().unwrap();
    super::address_from_json(json_args).unwrap();

    // without address
    let json_str = "[{\"amount\":100000, \"memo\":\"test memo\"}]";
    let json_args = json::parse(json_str).unwrap();
    let json_args = json_args.members().next().unwrap();
    assert!(matches!(
        super::address_from_json(json_args),
        Err(CommandError::MissingKey(_))
    ));

    // invalid address
    let json_str = "[{\"address\": 1, \
                    \"amount\":100000, \"memo\":\"test memo\"}]";
    let json_args = json::parse(json_str).unwrap();
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
        Some(memo_bytes_from_string("test memo".to_string()).unwrap())
    );

    // without memo
    let json_str = "[{\"address\":\"zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p\", \
                    \"amount\":100000}]";
    let json_args = json::parse(json_str).unwrap();
    let json_args = json_args.members().next().unwrap();
    assert_eq!(super::memo_from_json(json_args).unwrap(), None);
}

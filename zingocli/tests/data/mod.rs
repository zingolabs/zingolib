pub mod seeds {
    #[test]
    fn validate_seeds() {
        let abandon_art_seed = zcash_primitives::zip339::Mnemonic::from_entropy([0; 32])
            .unwrap()
            .to_string();
        assert_eq!(ABANDON_ART_SEED, abandon_art_seed);
        // TODO user get_zaddr_from_bip39seed to generate this address from that seed.
    }
    //Generate test seed
    pub const ABANDON_ART_SEED: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";
    pub const HOSPITAL_MUSEUM_SEED: &str = "hospital museum valve antique skate museum \
     unfold vocal weird milk scale social vessel identify \
     crowd hospital control album rib bulb path oven civil tank";
}
pub const REGSAP_ADDR_FROM_ABANDONART: &str =
    "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
pub mod config_template_fillers {
    pub mod zcashd {
        pub fn basic(rpcport: &str, extra: &str) -> String {
            format!("\
### Blockchain Configuration
regtest=1
nuparams=5ba81b19:1 # Overwinter
nuparams=76b809bb:1 # Sapling
nuparams=2bb40e60:1 # Blossom
nuparams=f5b9230b:1 # Heartwood
nuparams=e9ff75a6:1 # Canopy
nuparams=c2d6d0b4:1 # NU5


### MetaData Storage and Retrieval
# txindex:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#miscellaneous-options
txindex=1
# insightexplorer:
# https://zcash.readthedocs.io/en/latest/rtd_pages/insight_explorer.html?highlight=insightexplorer#additional-getrawtransaction-fields
insightexplorer=1
experimentalfeatures=1


### RPC Server Interface Options:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#json-rpc-options
rpcuser=xxxxxx
rpcpassword=xxxxxx
rpcport={rpcport}
rpcallowip=127.0.0.1

# Buried config option to allow non-canonical RPC-PORT:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#zcash-conf-guide
listen=0

{extra}")
        }
        pub fn funded(mineraddress: &str, rpcport: &str) -> String {
            basic(rpcport,
                &format!(
                    "### Zcashd Help provides documentation of the following:
                    mineraddress={mineraddress}
                    minetolocalwallet=0 # This is set to false so that we can mine to a wallet, other than the zcashd wallet."
                )
            )
        }
    }
    pub mod lightwalletd {

        pub fn basic(rpcport: &str) -> String {
            format! {"\
# # Default zingo lib lightwalletd conf YAML for regtest mode # #
grpc-bind-addr: 127.0.0.1:{rpcport}
cache-size: 10
log-file: ../logs/lwd.log
log-level: 10
zcash-conf-path: ../conf/zcash.conf

# example config for TLS
#tls-cert: /secrets/lightwallted/example-only-cert.pem
#tls-key: /secrets/lightwallted/example-only-cert.key"}
        }
    }
}
pub const BLOCK_SPAWNRATE_SECONDS: u32 = 75;
pub const SECONDS_PER_DAY: u32 = 60 * 60 * 24;

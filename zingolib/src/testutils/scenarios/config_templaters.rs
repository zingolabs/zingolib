/// TODO: Add Doc Comment Here!
pub mod zcashd {
    use zcash_primitives::consensus::NetworkUpgrade;

    /// TODO: Add Doc Comment Here!
    pub fn basic(
        rpcport: &str,
        regtest_network: &crate::config::RegtestNetwork,
        lightwalletd_feature: bool,
        extra: &str,
    ) -> String {
        let overwinter_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Overwinter)
            .unwrap();
        let sapling_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Sapling)
            .unwrap();
        let blossom_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Blossom)
            .unwrap();
        let heartwood_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Heartwood)
            .unwrap();
        let canopy_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Canopy)
            .unwrap();
        let orchard_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Nu5)
            .unwrap();
        let nu6_activation_height = regtest_network
            .activation_height(NetworkUpgrade::Nu6)
            .unwrap();

        let lightwalletd = if lightwalletd_feature {
            "lightwalletd=1"
        } else {
            ""
        };

        format!("\
### Blockchain Configuration
regtest=1
nuparams=5ba81b19:{overwinter_activation_height} # Overwinter
nuparams=76b809bb:{sapling_activation_height} # Sapling
nuparams=2bb40e60:{blossom_activation_height} # Blossom
nuparams=f5b9230b:{heartwood_activation_height} # Heartwood
nuparams=e9ff75a6:{canopy_activation_height} # Canopy
nuparams=c2d6d0b4:{orchard_activation_height} # NU5 (Orchard)
nuparams=c8e71055:{nu6_activation_height} # NU6

### MetaData Storage and Retrieval
# txindex:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#miscellaneous-options
txindex=1
# insightexplorer:
# https://zcash.readthedocs.io/en/latest/rtd_pages/insight_explorer.html?highlight=insightexplorer#additional-getrawtransaction-fields
insightexplorer=1
experimentalfeatures=1
{lightwalletd}

### RPC Server Interface Options:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#json-rpc-options
rpcuser=xxxxxx
rpcpassword=xxxxxx
rpcport={rpcport}
rpcallowip=127.0.0.1

# Buried config option to allow non-canonical RPC-PORT:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#zcash-conf-guide
listen=0

{extra}"
            )
    }

    /// TODO: Add Doc Comment Here!
    pub fn funded(
        mineraddress: &str,
        rpcport: &str,
        regtest_network: &crate::config::RegtestNetwork,
        lightwalletd_feature: bool,
    ) -> String {
        basic(rpcport, regtest_network, lightwalletd_feature,
                &format!("\
### Zcashd Help provides documentation of the following:
mineraddress={mineraddress}
minetolocalwallet=0 # This is set to false so that we can mine to a wallet, other than the zcashd wallet."
                )
            )
    }

    #[test]
    fn funded_zcashd_conf() {
        let regtest_network = crate::config::RegtestNetwork::new(1, 2, 3, 4, 5, 6, 7);
        assert_eq!(
                funded(
                    testvectors::REG_Z_ADDR_FROM_ABANDONART,
                    "1234",
                    &regtest_network,
                    true,
                ),
                format!("\
### Blockchain Configuration
regtest=1
nuparams=5ba81b19:1 # Overwinter
nuparams=76b809bb:2 # Sapling
nuparams=2bb40e60:3 # Blossom
nuparams=f5b9230b:4 # Heartwood
nuparams=e9ff75a6:5 # Canopy
nuparams=c2d6d0b4:6 # NU5 (Orchard)
nuparams=c8e71055:7 # NU6

### MetaData Storage and Retrieval
# txindex:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#miscellaneous-options
txindex=1
# insightexplorer:
# https://zcash.readthedocs.io/en/latest/rtd_pages/insight_explorer.html?highlight=insightexplorer#additional-getrawtransaction-fields
insightexplorer=1
experimentalfeatures=1
lightwalletd=1

### RPC Server Interface Options:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#json-rpc-options
rpcuser=xxxxxx
rpcpassword=xxxxxx
rpcport=1234
rpcallowip=127.0.0.1

# Buried config option to allow non-canonical RPC-PORT:
# https://zcash.readthedocs.io/en/latest/rtd_pages/zcash_conf_guide.html#zcash-conf-guide
listen=0

### Zcashd Help provides documentation of the following:
mineraddress=zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p
minetolocalwallet=0 # This is set to false so that we can mine to a wallet, other than the zcashd wallet."
                )
            );
    }
}

/// TODO: Add Doc Comment Here!
pub mod lightwalletd {
    /// TODO: Add Doc Comment Here!
    pub fn basic(rpcport: &str) -> String {
        format!(
            "\
# # Default zingo lib lightwalletd conf YAML for regtest mode # #
grpc-bind-addr: 127.0.0.1:{rpcport}
cache-size: 10
log-file: ../logs/lwd.log
log-level: 10
zcash-conf-path: ../conf/zcash.conf

# example config for TLS
#tls-cert: /secrets/lightwallted/example-only-cert.pem
#tls-key: /secrets/lightwallted/example-only-cert.key"
        )
    }

    #[test]
    fn basic_lightwalletd_conf() {
        assert_eq!(
            basic("1234"),
            format!(
                "\
# # Default zingo lib lightwalletd conf YAML for regtest mode # #
grpc-bind-addr: 127.0.0.1:1234
cache-size: 10
log-file: ../logs/lwd.log
log-level: 10
zcash-conf-path: ../conf/zcash.conf

# example config for TLS
#tls-cert: /secrets/lightwallted/example-only-cert.pem
#tls-key: /secrets/lightwallted/example-only-cert.key"
            )
        )
    }
}

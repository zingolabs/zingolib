pub const ZCASHD_TEMPLATE: &str = "\
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
rpcport=18232
rpcallowip=127.0.0.1


### Zcashd Help provides documentation of the following:
mineraddress=zregtestsapling1fkc26vpg566hgnx33n5uvgye4neuxt4358k68atnx78l5tg2dewdycesmr4m5pn56ffzsa7lyj6
minetolocalwallet=0 # This is set to false so that we can mine to a wallet, other than the zcashd wallet.";

pub const SECRET_SPEND_AUTH_SAPLING: &str = "\
secret-extended-key-regtest1qwxwxvvsqqqqpqpscrwl4x0sahmtm7j3wgl2q4n44c8wzkdf5q04wue4wpjsvtw9js33wjet4582cwkhwnrgu82nzps7v7mnk3htzac0qaskl4vjlacs8xstgfedq0yhp8t2t8k8r28telx77vkc9jx506wcl7yxvucwjys2rk59627kv92kgqp8nqujmmt3vnln7ytlwjm53euylkyruft54lg34c7ne2w6sc9c2wam3yne5t2jvh7458hezwyfaxljxvunqwwlancespz6n";

pub const SAPLING_ADDRESS_FROM_SPEND_AUTH: &str = "\
zregtestsapling1fkc26vpg566hgnx33n5uvgye4neuxt4358k68atnx78l5tg2dewdycesmr4m5pn56ffzsa7lyj6";

pub fn fill_conf_template(mineraddress: &str) -> String {
    return format!("\
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
rpcport=18232
rpcallowip=127.0.0.1


### Zcashd Help provides documentation of the following:
mineraddress={mineraddress}
minetolocalwallet=0 # This is set to false so that we can mine to a wallet, other than the zcashd wallet.");
}

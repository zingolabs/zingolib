set -e -x
rm -rf testnet_zebrad.toml
zebrad generate --output-file testnet_zebrad.toml 
sed -i 's/Mainnet/Testnet/g' testnet_zebrad.toml
sed -i 's/listen_addr = "0.0.0.0:8233"/listen_addr = "0.0.0.0:18233"/g' testnet_zebrad.toml

zebrad --config testnet_zebrad.toml start

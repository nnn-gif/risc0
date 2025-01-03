// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// This application demonstrates how to send an off-chain proof request
// to the Bonsai proving service and publish the received proofs directly
// to your deployed app contract.

use alloy::{
    network::EthereumWallet, providers::ProviderBuilder, rpc::types::TransactionReceipt, signers::local::PrivateKeySigner, sol_types::{SolCall, SolValue}
};
use std::str::FromStr;
use alloy_primitives::hex;

use alloy_primitives::{Address, U256,U160};
use alloy_primitives::aliases::U24;
use anyhow::{ensure, Context, Result};
use clap::Parser;
use erc20_counter_methods::BALANCE_OF_ELF;
use erc20_counter_methods::SIMULATE_UNI_ELF;
use erc20_counter_methods::SIMULATE_UNI_ID;

use risc0_ethereum_contracts::encode_seal;
use risc0_steel::{
    ethereum::{EthEvmEnv, ETH_SEPOLIA_CHAIN_SPEC,ETH_MAINNET_CHAIN_SPEC},
    host::BlockNumberOrTag,
    Commitment, Contract,
};
use risc0_zkvm::{default_prover, sha::Digest, ExecutorEnv, ProverOpts, VerifierContext};
use tokio::task;
use tracing_subscriber::EnvFilter;
use url::Url;

alloy::sol! {
    /// Interface to be called by the guest.
    interface IERC20 {
        function balanceOf(address account) external view returns (uint);
    }
    interface IQuoter {
        function quoteExactInputSingle(
            address tokenIn,
            address tokenOut,
            uint24 fee,
            uint256 amountIn,
            uint160 sqrtPriceLimitX96
        ) external returns (uint256 amountOut);

        function factory() returns (address);
    }

    /// Data committed to by the guest.
    struct Journal {
        Commitment commitment;
        // address fromAddress;
        address tokenIn;
        // address tokenOut;
        // uint256 price;

 
    }
}

alloy::sol!(
    #[sol(rpc, all_derives)]
    "../contracts/src/IDexVerify.sol"
);

/// Simple program to create a proof to increment the Counter contract.
#[derive(Parser)]
struct Args {
    /// Ethereum private key
    #[clap(long, env)]
    eth_wallet_private_key: PrivateKeySigner,

    /// Ethereum RPC endpoint URL
    #[clap(long, env)]
    eth_rpc_url: Url,

    /// Beacon API endpoint URL
    ///
    /// Steel uses a beacon block commitment instead of the execution block.
    /// This allows proofs to be validated using the EIP-4788 beacon roots contract.
    #[clap(long, env)]
    #[cfg(any(feature = "beacon", feature = "history"))]
    beacon_api_url: Url,

    /// Ethereum block to use as the state for the contract call
    #[clap(long, env, default_value_t = BlockNumberOrTag::Parent)]
    execution_block: BlockNumberOrTag,

    /// Ethereum block to use for the beacon block commitment.
    #[clap(long, env)]
    #[cfg(feature = "history")]
    commitment_block: BlockNumberOrTag,

    /// Address of the Counter verifier contract
    #[clap(long)]
    counter_address: Address,

    /// Address of the ERC20 token contract
    #[clap(long)]
    simulation_contract: Address,


    #[clap(long)]
    token_in: Address,

    #[clap(long)]
    token_out: Address,

    #[clap(long)]
    from_address: Address,

 

    /// Address to query the token balance of
    #[clap(long)]
    account: Address,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    // Parse the command line arguments.
    let args = Args::try_parse()?;

    // Create an alloy provider for that private key and URL.
    let wallet = EthereumWallet::from(args.eth_wallet_private_key);
    let provider = ProviderBuilder::new()
        .with_recommended_fillers()
        // .wallet(wallet)
        .on_http(args.eth_rpc_url);

        let sepolia_url = Url::parse("https://ethereum-sepolia-rpc.publicnode.com").unwrap();

        let providerSepolia = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(wallet)
        .on_http(sepolia_url);

    #[cfg(feature = "beacon")]
    log::info!("Beacon commitment to block {}", args.execution_block);
    #[cfg(feature = "history")]
    log::info!("History commitment to block {}", args.commitment_block);

    let builder = EthEvmEnv::builder()
        .provider(provider.clone())
        .block_number_or_tag(args.execution_block);
    #[cfg(any(feature = "beacon", feature = "history"))]
    let builder = builder.beacon_api(args.beacon_api_url);
    #[cfg(feature = "history")]
    let builder = builder.commitment_block(args.commitment_block);

    let mut env = builder.build().await?;
    //  The `with_chain_spec` method is used to specify the chain configuration.
    env = env.with_chain_spec(&ETH_MAINNET_CHAIN_SPEC);
    

    // let token_in = Address::parse_checksummed(args.token_in, None).unwrap();
    // let token_out = Address::parse_checksummed(args.token_out, None).unwrap();
    // let from_address = Address::parse_checksummed(args.from_address, None).unwrap();


    // Prepare the function call
    let quote_call = IQuoter::quoteExactInputSingleCall {
        tokenIn: args.token_in, tokenOut: args.token_out,fee: U24::from(3000),amountIn: U256::from(1000),sqrtPriceLimitX96: U160::from(0)
    };

 
    

    // let factory_call = IQuoter::factoryCall {
        let mut gas_price: U256 = "8798335655".parse().unwrap();

    // };
    // Preflight the call to prepare the input that is required to execute the function in
    // the guest without RPC access. It also returns the result of the call.
    let mut contract = Contract::preflight(args.simulation_contract, &mut env);
    let returns = contract.call_builder(&quote_call).from(args.from_address).call().await?.amountOut;

    println!("Returns:------------------------------ {}", returns);
    // assert!(returns >= U256::from(1));

    // Finally, construct the input from the environment.
    // There are two options: Use EIP-4788 for verification by providing a Beacon API endpoint,
    // or use the regular `blockhash' opcode.
    let evm_input = env.into_input().await?;

    // Create the steel proof.
    let prove_info = task::spawn_blocking(move || {
        let env = ExecutorEnv::builder()
            .write(&evm_input)?
            .write(&args.simulation_contract)?
            .write(&args.token_in)?
            .write(&args.token_out)?
            .write(&args.from_address)?

            .write(&args.account)?
            .build()
            .unwrap();

        default_prover().prove_with_ctx(
            env,
            &VerifierContext::default(),
            SIMULATE_UNI_ELF,
            &ProverOpts::groth16(),
        )
    })
    .await?
    .context("failed to create proof")?;
    let receipt = prove_info.receipt;
    let journal = &receipt.journal.bytes;

    // Decode and log the commitment
    let journal = Journal::abi_decode(journal, true).context("invalid journal")?;
    log::debug!("Steel commitment: {:?}", journal.commitment);

    // ABI encode the seal.
    let seal = encode_seal(&receipt).context("invalid receipt")?;

    // Create an alloy instance of the Counter contract.
    let contract = IDexVerify::new(args.counter_address, &providerSepolia);


    //  log::info!(
    //     "from {}   ...",
    //     journal.fromAddress
    // );
    log::info!(
        "tokenIn {}   ...",
        journal.tokenIn
    );
    // log::info!(
    //     "tokenOut {}   ...",
    //     journal.tokenOut
    // );

    log::info!(
        "price {}   ...",
        returns
    );

    println!("Journal (hex) = {}", hex::encode(&receipt.journal.bytes));

    println!("seal (hex) = {}", hex::encode(&seal));

 

    

   

    // Call ICounter::imageID() to check that the contract has been deployed correctly.
    let contract_image_id = Digest::from(contract.imageID().call().await?._0.0);

    println!("image id = {}", hex::encode(Digest::from(SIMULATE_UNI_ID)));

    ensure!(contract_image_id == Digest::from(SIMULATE_UNI_ID));


    let vf =  contract.verify(receipt.journal.bytes.into(), seal.into()).call().await?._0;



 
      println!("vf  = {}", vf);


    // Call the increment function of the contract and wait for confirmation.
    // log::info!(
    //     "Sending Tx calling {} Function of {:#}...",
    //     ICounter::incrementCall::SIGNATURE,
    //     contract.address()
    // );
    // let call_builder = contract.increment(receipt.journal.bytes.into(), seal.into());
    // log::debug!("Send {} {}", contract.address(), call_builder.calldata());
    // let pending_tx = call_builder.send().await?;
    // let tx_hash = *pending_tx.tx_hash();
    // let receipt = pending_tx
    //     .get_receipt()
    //     .await
    //     .with_context(|| format!("transaction did not confirm: {}", tx_hash))?;
    // ensure!(receipt.status(), "transaction failed: {}", tx_hash);

    Ok(())
}

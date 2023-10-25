use {
    e2e::{setup::*, tx, tx_value},
    ethcontract::U256,
    model::quote::{OrderQuoteRequest, OrderQuoteSide, SellAmount},
    number::nonzero::U256 as NonZeroU256,
    shared::ethrpc::Web3,
};

#[tokio::test]
#[ignore]
async fn local_node_uses_stale_liquidity() {
    run_test(uses_stale_liquidity).await;
}

async fn uses_stale_liquidity(web3: Web3, db: DbUrl) {
    tracing::info!("Setting up chain state.");
    let mut onchain = OnchainComponents::deploy(web3.clone()).await;

    let [solver] = onchain.make_solvers(to_wei(10)).await;
    let [trader] = onchain.make_accounts(to_wei(2)).await;
    let [token] = onchain
        .deploy_tokens_with_weth_uni_v2_pools(to_wei(1_000), to_wei(1_000))
        .await;

    tx!(
        trader.account(),
        onchain
            .contracts()
            .weth
            .approve(onchain.contracts().allowance, to_wei(1))
    );
    tx_value!(
        trader.account(),
        to_wei(1),
        onchain.contracts().weth.deposit()
    );

    tracing::info!("Starting services.");
    let solver_endpoint = colocation::start_solver(onchain.contracts().weth.address()).await;
    let driver_url = colocation::start_driver(onchain.contracts(), &solver_endpoint, &solver).await;

    let services = Services::new(onchain.contracts(), db).await;
    services.start_autopilot(vec![
        "--enable-colocation=true".to_string(),
        format!("--drivers=solver|{}/test_solver", driver_url.as_str()),
    ]);
    services
        .start_api(vec![format!(
            "--price-estimation-drivers=solver|{}/test_solver",
            driver_url.as_str()
        )])
        .await;

    let quote = OrderQuoteRequest {
        from: trader.address(),
        sell_token: onchain.contracts().weth.address(),
        buy_token: token.address(),
        side: OrderQuoteSide::Sell {
            sell_amount: SellAmount::AfterFee {
                value: NonZeroU256::new(to_wei(1)).unwrap(),
            },
        },
        ..Default::default()
    };

    tracing::info!("performining initial quote");
    let first = services.submit_quote(&quote).await.unwrap();

    // Now, we want to manually unbalance the pools and assert that the quote
    // doesn't change (as the price estimation will use stale pricing data).
    onchain
        .mint_token_to_weth_uni_v2_pool(&token, to_wei(1_000))
        .await;

    tracing::info!("performining second quote, which should match first");
    let second = services.submit_quote(&quote).await.unwrap();
    assert_eq!(first.quote.buy_amount, second.quote.buy_amount);

    tracing::info!("waiting for liquidity state to update");
    wait_for_condition(TIMEOUT, || async {
        let next = services.submit_quote(&quote).await.unwrap();
        next.quote.buy_amount != first.quote.buy_amount
    })
    .await
    .unwrap();
}

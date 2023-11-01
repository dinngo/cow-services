use {
    database::order_events::OrderEventLabel,
    e2e::{setup::*, tx, tx_value},
    ethcontract::U256,
    model::{
        order::{OrderCreation, OrderKind},
        signature::EcdsaSigningScheme,
    },
    secp256k1::SecretKey,
    web3::signing::SecretKeyRef,
};

#[tokio::test]
#[ignore]
async fn local_node_test() {
    run_test(test).await;
}

async fn test(web3: Web3, db: Db) {
    tracing::info!("Setting up chain state.");
    let mut onchain = OnchainComponents::deploy(web3).await;

    let [solver] = onchain.make_solvers(to_wei(10)).await;
    let [trader] = onchain.make_accounts(to_wei(10)).await;
    let [token] = onchain
        .deploy_tokens_with_weth_uni_v2_pools(to_wei(1_000), to_wei(1_000))
        .await;

    tx!(
        trader.account(),
        onchain
            .contracts()
            .weth
            .approve(onchain.contracts().allowance, to_wei(3))
    );
    tx_value!(
        trader.account(),
        to_wei(3),
        onchain.contracts().weth.deposit()
    );

    tracing::info!("Starting services.");
    let solver_endpoint = colocation::start_solver(onchain.contracts().weth.address()).await;
    let driver_url = colocation::start_driver(
        onchain.contracts(),
        &solver_endpoint,
        &solver,
        &format!("http://localhost:{}", onchain.rpc_port()),
    )
    .await;

    let services = Services::new(&onchain, db).await;
    services.start_autopilot(vec![
        "--enable-colocation=true".to_string(),
        format!("--drivers=solver|{}test_solver", driver_url.as_str()),
    ]);
    services.start_api(vec![]).await;

    tracing::info!("Placing order");
    let order = OrderCreation {
        sell_token: onchain.contracts().weth.address(),
        sell_amount: to_wei(2),
        fee_amount: to_wei(1),
        buy_token: token.address(),
        buy_amount: to_wei(1),
        valid_to: model::time::now_in_epoch_seconds() + 300,
        kind: OrderKind::Buy,
        ..Default::default()
    }
    .sign(
        EcdsaSigningScheme::Eip712,
        &onchain.contracts().domain_separator,
        SecretKeyRef::from(&SecretKey::from_slice(trader.private_key()).unwrap()),
    );
    let uid = services.create_order(&order).await.unwrap();

    tracing::info!("Withdrawing WETH to render the order invalid due to insufficient funds");
    tx!(
        trader.account(),
        onchain.contracts().weth.withdraw(to_wei(3))
    );

    let order_is_invalid = || async {
        let events = crate::database::events_of_order(services.db(), &uid).await;
        events.last().map(|e| e.label) == Some(OrderEventLabel::Invalid)
    };
    wait_for_condition(TIMEOUT, order_is_invalid).await.unwrap();
}

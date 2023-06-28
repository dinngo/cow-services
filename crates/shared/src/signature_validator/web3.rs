use {
    super::{check_erc1271_result, SignatureCheck, SignatureValidating, SignatureValidationError},
    crate::{
        ethcontract_error::EthcontractErrorType,
        ethrpc::{Web3, MAX_BATCH_SIZE},
    },
    contracts::ERC1271SignatureValidator,
    ethcontract::{
        batch::CallBatch,
        errors::{ExecutionError, MethodError},
        Bytes,
    },
    futures::future,
};

const TRANSACTION_INITIALIZATION_GAS_AMOUNT: u64 = 21_000u64;

pub struct Web3SignatureValidator {
    web3: Web3,
}

impl Web3SignatureValidator {
    pub fn new(web3: Web3) -> Self {
        Self { web3 }
    }
}

#[async_trait::async_trait]
impl SignatureValidating for Web3SignatureValidator {
    async fn validate_signatures(
        &self,
        checks: Vec<SignatureCheck>,
    ) -> Vec<Result<(), SignatureValidationError>> {
        let mut batch = CallBatch::new(self.web3.transport().clone());
        let calls = checks
            .into_iter()
            .map(|check| {
                if !check.interactions.is_empty() {
                    tracing::warn!(
                        ?check,
                        "verifying ERC-1271 signatures with interactions is not fully supported"
                    );
                }

                let instance = ERC1271SignatureValidator::at(&self.web3, check.signer);
                let call = instance
                    .is_valid_signature(Bytes(check.hash), Bytes(check.signature))
                    .batch_call(&mut batch);

                async move { check_erc1271_result(call.await?) }
            })
            .collect::<Vec<_>>();

        batch.execute_all(MAX_BATCH_SIZE).await;
        future::join_all(calls).await
    }

    async fn validate_signature_and_get_additional_gas(
        &self,
        check: SignatureCheck,
    ) -> Result<u64, SignatureValidationError> {
        if !check.interactions.is_empty() {
            tracing::warn!(
                ?check,
                "verifying ERC-1271 signatures with interactions is not fully supported"
            );
        }

        let instance = ERC1271SignatureValidator::at(&self.web3, check.signer);
        let check = instance.is_valid_signature(Bytes(check.hash), Bytes(check.signature.clone()));

        let (result, gas_estimate) =
            futures::join!(check.clone().call(), check.m.tx.estimate_gas());

        check_erc1271_result(result?)?;

        // Adjust the estimate we receive by the fixed transaction gas cost.
        // This is because this cost is not paid by an internal call, but by
        // the root transaction only.
        Ok(gas_estimate?.as_u64() - TRANSACTION_INITIALIZATION_GAS_AMOUNT)
    }
}

impl From<MethodError> for SignatureValidationError {
    fn from(err: MethodError) -> Self {
        // Classify "contract" errors as invalid signatures instead of node
        // errors (which may be temporary). This can happen if there is ABI
        // compability issues or calling an EOA instead of a SC.
        match EthcontractErrorType::classify(&err) {
            EthcontractErrorType::Contract => Self::Invalid,
            _ => Self::Other(err.into()),
        }
    }
}

impl From<ExecutionError> for SignatureValidationError {
    fn from(err: ExecutionError) -> Self {
        match EthcontractErrorType::classify(&err) {
            EthcontractErrorType::Contract => Self::Invalid,
            _ => Self::Other(err.into()),
        }
    }
}
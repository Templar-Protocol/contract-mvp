"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.AccountMultisig = void 0;
const transactions_1 = require("@near-js/transactions");
const account_1 = require("./account");
const constants_1 = require("./constants");
const types_1 = require("./types");
const { deployContract, functionCall } = transactions_1.actionCreators;
var MultisigCodeStatus;
(function (MultisigCodeStatus) {
    MultisigCodeStatus[MultisigCodeStatus["INVALID_CODE"] = 0] = "INVALID_CODE";
    MultisigCodeStatus[MultisigCodeStatus["VALID_CODE"] = 1] = "VALID_CODE";
    MultisigCodeStatus[MultisigCodeStatus["UNKNOWN_CODE"] = 2] = "UNKNOWN_CODE";
})(MultisigCodeStatus || (MultisigCodeStatus = {}));
// in memory request cache for node w/o localStorage
const storageFallback = {
    [constants_1.MULTISIG_STORAGE_KEY]: null
};
class AccountMultisig extends account_1.Account {
    constructor(connection, accountId, options) {
        super(connection, accountId);
        this.storage = options.storage;
        this.onAddRequestResult = options.onAddRequestResult;
    }
    signAndSendTransactionWithAccount(receiverId, actions) {
        const _super = Object.create(null, {
            signAndSendTransaction: { get: () => super.signAndSendTransaction }
        });
        return __awaiter(this, void 0, void 0, function* () {
            return _super.signAndSendTransaction.call(this, { receiverId, actions });
        });
    }
    signAndSendTransaction({ receiverId, actions }) {
        const _super = Object.create(null, {
            signAndSendTransaction: { get: () => super.signAndSendTransaction }
        });
        return __awaiter(this, void 0, void 0, function* () {
            const { accountId } = this;
            const args = Buffer.from(JSON.stringify({
                request: {
                    receiver_id: receiverId,
                    actions: convertActions(actions, accountId, receiverId)
                }
            }));
            let result;
            try {
                result = yield _super.signAndSendTransaction.call(this, {
                    receiverId: accountId,
                    actions: [
                        functionCall('add_request_and_confirm', args, constants_1.MULTISIG_GAS, constants_1.MULTISIG_DEPOSIT)
                    ]
                });
            }
            catch (e) {
                if (e.toString().includes('Account has too many active requests. Confirm or delete some')) {
                    yield this.deleteUnconfirmedRequests();
                    return yield this.signAndSendTransaction({ receiverId, actions });
                }
                throw e;
            }
            // TODO: Are following even needed? Seems like it throws on error already
            if (!result.status) {
                throw new Error('Request failed');
            }
            const status = Object.assign({}, result.status);
            if (!status.SuccessValue || typeof status.SuccessValue !== 'string') {
                throw new Error('Request failed');
            }
            this.setRequest({
                accountId,
                actions,
                requestId: parseInt(Buffer.from(status.SuccessValue, 'base64').toString('ascii'), 10)
            });
            if (this.onAddRequestResult) {
                yield this.onAddRequestResult(result);
            }
            // NOTE there is no await on purpose to avoid blocking for 2fa
            this.deleteUnconfirmedRequests();
            return result;
        });
    }
    /*
     * This method submits a canary transaction that is expected to always fail in order to determine whether the contract currently has valid multisig state
     * and whether it is initialized. The canary transaction attempts to delete a request at index u32_max and will go through if a request exists at that index.
     * a u32_max + 1 and -1 value cannot be used for the canary due to expected u32 error thrown before deserialization attempt.
     */
    checkMultisigCodeAndStateStatus(contractBytes) {
        const _super = Object.create(null, {
            signAndSendTransaction: { get: () => super.signAndSendTransaction }
        });
        return __awaiter(this, void 0, void 0, function* () {
            const u32_max = 4294967295;
            const validCodeStatusIfNoDeploy = contractBytes ? MultisigCodeStatus.UNKNOWN_CODE : MultisigCodeStatus.VALID_CODE;
            try {
                if (contractBytes) {
                    yield _super.signAndSendTransaction.call(this, {
                        receiverId: this.accountId, actions: [
                            deployContract(contractBytes),
                            functionCall('delete_request', { request_id: u32_max }, constants_1.MULTISIG_GAS, constants_1.MULTISIG_DEPOSIT)
                        ]
                    });
                }
                else {
                    yield this.deleteRequest(u32_max);
                }
                return { codeStatus: MultisigCodeStatus.VALID_CODE, stateStatus: types_1.MultisigStateStatus.VALID_STATE };
            }
            catch (e) {
                if (new RegExp(types_1.MultisigDeleteRequestRejectionError.CANNOT_DESERIALIZE_STATE).test(e && e.kind && e.kind.ExecutionError)) {
                    return { codeStatus: validCodeStatusIfNoDeploy, stateStatus: types_1.MultisigStateStatus.INVALID_STATE };
                }
                else if (new RegExp(types_1.MultisigDeleteRequestRejectionError.MULTISIG_NOT_INITIALIZED).test(e && e.kind && e.kind.ExecutionError)) {
                    return { codeStatus: validCodeStatusIfNoDeploy, stateStatus: types_1.MultisigStateStatus.STATE_NOT_INITIALIZED };
                }
                else if (new RegExp(types_1.MultisigDeleteRequestRejectionError.NO_SUCH_REQUEST).test(e && e.kind && e.kind.ExecutionError)) {
                    return { codeStatus: validCodeStatusIfNoDeploy, stateStatus: types_1.MultisigStateStatus.VALID_STATE };
                }
                else if (new RegExp(types_1.MultisigDeleteRequestRejectionError.METHOD_NOT_FOUND).test(e && e.message)) {
                    // not reachable if transaction included a deploy
                    return { codeStatus: MultisigCodeStatus.INVALID_CODE, stateStatus: types_1.MultisigStateStatus.UNKNOWN_STATE };
                }
                throw e;
            }
        });
    }
    deleteRequest(request_id) {
        return super.signAndSendTransaction({
            receiverId: this.accountId,
            actions: [functionCall('delete_request', { request_id }, constants_1.MULTISIG_GAS, constants_1.MULTISIG_DEPOSIT)]
        });
    }
    deleteAllRequests() {
        return __awaiter(this, void 0, void 0, function* () {
            const request_ids = yield this.getRequestIds();
            if (request_ids.length) {
                yield Promise.all(request_ids.map((id) => this.deleteRequest(id)));
            }
        });
    }
    deleteUnconfirmedRequests() {
        const _super = Object.create(null, {
            signAndSendTransaction: { get: () => super.signAndSendTransaction }
        });
        return __awaiter(this, void 0, void 0, function* () {
            // TODO: Delete in batch, don't delete unexpired
            // TODO: Delete in batch, don't delete unexpired (can reduce gas usage dramatically)
            const request_ids = yield this.getRequestIds();
            const { requestId } = this.getRequest();
            for (const requestIdToDelete of request_ids) {
                if (requestIdToDelete == requestId) {
                    continue;
                }
                try {
                    yield _super.signAndSendTransaction.call(this, {
                        receiverId: this.accountId,
                        actions: [functionCall('delete_request', { request_id: requestIdToDelete }, constants_1.MULTISIG_GAS, constants_1.MULTISIG_DEPOSIT)]
                    });
                }
                catch (e) {
                    console.warn('Attempt to delete an earlier request before 15 minutes failed. Will try again.');
                }
            }
        });
    }
    // helpers
    getRequestIds() {
        return __awaiter(this, void 0, void 0, function* () {
            // TODO: Read requests from state to allow filtering by expiration time
            // TODO: https://github.com/near/core-contracts/blob/305d1db4f4f2cf5ce4c1ef3479f7544957381f11/multisig/src/lib.rs#L84
            return this.viewFunction({
                contractId: this.accountId,
                methodName: 'list_request_ids',
            });
        });
    }
    getRequest() {
        if (this.storage) {
            return JSON.parse(this.storage.getItem(constants_1.MULTISIG_STORAGE_KEY) || '{}');
        }
        return storageFallback[constants_1.MULTISIG_STORAGE_KEY];
    }
    setRequest(data) {
        if (this.storage) {
            return this.storage.setItem(constants_1.MULTISIG_STORAGE_KEY, JSON.stringify(data));
        }
        storageFallback[constants_1.MULTISIG_STORAGE_KEY] = data;
    }
}
exports.AccountMultisig = AccountMultisig;
const convertPKForContract = (pk) => pk.toString().replace('ed25519:', '');
const convertActions = (actions, accountId, receiverId) => actions.map((a) => {
    const type = a.enum;
    const { gas, publicKey, methodName, args, deposit, accessKey, code } = a[type];
    const action = {
        type: type[0].toUpperCase() + type.substr(1),
        gas: (gas && gas.toString()) || undefined,
        public_key: (publicKey && convertPKForContract(publicKey)) || undefined,
        method_name: methodName,
        args: (args && Buffer.from(args).toString('base64')) || undefined,
        code: (code && Buffer.from(code).toString('base64')) || undefined,
        amount: (deposit && deposit.toString()) || undefined,
        deposit: (deposit && deposit.toString()) || '0',
        permission: undefined,
    };
    if (accessKey) {
        if (receiverId === accountId && accessKey.permission.enum !== 'fullAccess') {
            action.permission = {
                receiver_id: accountId,
                allowance: constants_1.MULTISIG_ALLOWANCE.toString(),
                method_names: constants_1.MULTISIG_CHANGE_METHODS,
            };
        }
        if (accessKey.permission.enum === 'functionCall') {
            const { receiverId: receiver_id, methodNames: method_names, allowance } = accessKey.permission.functionCall;
            action.permission = {
                receiver_id,
                allowance: (allowance && allowance.toString()) || undefined,
                method_names
            };
        }
    }
    return action;
});

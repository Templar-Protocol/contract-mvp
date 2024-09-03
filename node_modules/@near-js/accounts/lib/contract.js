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
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.Contract = void 0;
const utils_1 = require("@near-js/utils");
const types_1 = require("@near-js/types");
const ajv_1 = __importDefault(require("ajv"));
const ajv_formats_1 = __importDefault(require("ajv-formats"));
const bn_js_1 = __importDefault(require("bn.js"));
const depd_1 = __importDefault(require("depd"));
const near_abi_1 = require("near-abi");
const errors_1 = require("./errors");
// Makes `function.name` return given name
function nameFunction(name, body) {
    return {
        [name](...args) {
            return body(...args);
        }
    }[name];
}
function validateArguments(args, abiFunction, ajv, abiRoot) {
    var _a;
    if (!isObject(args))
        return;
    if (abiFunction.params && abiFunction.params.serialization_type !== near_abi_1.AbiSerializationType.Json) {
        throw new errors_1.UnsupportedSerializationError(abiFunction.name, abiFunction.params.serialization_type);
    }
    if (abiFunction.result && abiFunction.result.serialization_type !== near_abi_1.AbiSerializationType.Json) {
        throw new errors_1.UnsupportedSerializationError(abiFunction.name, abiFunction.result.serialization_type);
    }
    const params = ((_a = abiFunction.params) === null || _a === void 0 ? void 0 : _a.args) || [];
    for (const p of params) {
        const arg = args[p.name];
        const typeSchema = p.type_schema;
        typeSchema.definitions = abiRoot.body.root_schema.definitions;
        const validate = ajv.compile(typeSchema);
        if (!validate(arg)) {
            throw new errors_1.ArgumentSchemaError(p.name, validate.errors);
        }
    }
    // Check there are no extra unknown arguments passed
    for (const argName of Object.keys(args)) {
        const param = params.find((p) => p.name === argName);
        if (!param) {
            throw new errors_1.UnknownArgumentError(argName, params.map((p) => p.name));
        }
    }
}
function createAjv() {
    // Strict mode is disabled for now as it complains about unknown formats. We need to
    // figure out if we want to support a fixed set of formats. `uint32` and `uint64`
    // are added explicitly just to reduce the amount of warnings as these are very popular
    // types.
    const ajv = new ajv_1.default({
        strictSchema: false,
        formats: {
            uint32: true,
            uint64: true
        }
    });
    (0, ajv_formats_1.default)(ajv);
    return ajv;
}
const isUint8Array = (x) => x && x.byteLength !== undefined && x.byteLength === x.length;
const isObject = (x) => Object.prototype.toString.call(x) === '[object Object]';
/**
 * Defines a smart contract on NEAR including the change (mutable) and view (non-mutable) methods
 *
 * @see [https://docs.near.org/tools/near-api-js/quick-reference#contract](https://docs.near.org/tools/near-api-js/quick-reference#contract)
 * @example
 * ```js
 * import { Contract } from 'near-api-js';
 *
 * async function contractExample() {
 *   const methodOptions = {
 *     viewMethods: ['getMessageByAccountId'],
 *     changeMethods: ['addMessage']
 *   };
 *   const contract = new Contract(
 *     wallet.account(),
 *     'contract-id.testnet',
 *     methodOptions
 *   );
 *
 *   // use a contract view method
 *   const messages = await contract.getMessages({
 *     accountId: 'example-account.testnet'
 *   });
 *
 *   // use a contract change method
 *   await contract.addMessage({
 *      meta: 'some info',
 *      callbackUrl: 'https://example.com/callback',
 *      args: { text: 'my message' },
 *      amount: 1
 *   })
 * }
 * ```
 */
class Contract {
    /**
     * @param account NEAR account to sign change method transactions
     * @param contractId NEAR account id where the contract is deployed
     * @param options NEAR smart contract methods that your application will use. These will be available as `contract.methodName`
     */
    constructor(account, contractId, options) {
        this.account = account;
        this.contractId = contractId;
        const { viewMethods = [], changeMethods = [], abi: abiRoot } = options;
        let viewMethodsWithAbi = viewMethods.map((name) => ({ name, abi: null }));
        let changeMethodsWithAbi = changeMethods.map((name) => ({ name, abi: null }));
        if (abiRoot) {
            if (viewMethodsWithAbi.length > 0 || changeMethodsWithAbi.length > 0) {
                throw new errors_1.ConflictingOptions();
            }
            viewMethodsWithAbi = abiRoot.body.functions
                .filter((m) => m.kind === near_abi_1.AbiFunctionKind.View)
                .map((m) => ({ name: m.name, abi: m }));
            changeMethodsWithAbi = abiRoot.body.functions
                .filter((methodAbi) => methodAbi.kind === near_abi_1.AbiFunctionKind.Call)
                .map((methodAbi) => ({ name: methodAbi.name, abi: methodAbi }));
        }
        const ajv = createAjv();
        viewMethodsWithAbi.forEach(({ name, abi }) => {
            Object.defineProperty(this, name, {
                writable: false,
                enumerable: true,
                value: nameFunction(name, (args = {}, options = {}, ...ignored) => __awaiter(this, void 0, void 0, function* () {
                    if (ignored.length || !(isObject(args) || isUint8Array(args)) || !isObject(options)) {
                        throw new types_1.PositionalArgsError();
                    }
                    if (abi) {
                        validateArguments(args, abi, ajv, abiRoot);
                    }
                    return this.account.viewFunction(Object.assign({ contractId: this.contractId, methodName: name, args }, options));
                }))
            });
        });
        changeMethodsWithAbi.forEach(({ name, abi }) => {
            Object.defineProperty(this, name, {
                writable: false,
                enumerable: true,
                value: nameFunction(name, (...args) => __awaiter(this, void 0, void 0, function* () {
                    if (args.length && (args.length > 3 || !(isObject(args[0]) || isUint8Array(args[0])))) {
                        throw new types_1.PositionalArgsError();
                    }
                    if (args.length > 1 || !(args[0] && args[0].args)) {
                        const deprecate = (0, depd_1.default)('contract.methodName(args, gas, amount)');
                        deprecate('use `contract.methodName({ args, gas?, amount?, callbackUrl?, meta? })` instead');
                        args[0] = {
                            args: args[0],
                            gas: args[1],
                            amount: args[2]
                        };
                    }
                    if (abi) {
                        validateArguments(args[0].args, abi, ajv, abiRoot);
                    }
                    return this._changeMethod(Object.assign({ methodName: name }, args[0]));
                }))
            });
        });
    }
    _changeMethod({ args, methodName, gas, amount, meta, callbackUrl }) {
        return __awaiter(this, void 0, void 0, function* () {
            validateBNLike({ gas, amount });
            const rawResult = yield this.account.functionCall({
                contractId: this.contractId,
                methodName,
                args,
                gas,
                attachedDeposit: amount,
                walletMeta: meta,
                walletCallbackUrl: callbackUrl
            });
            return (0, utils_1.getTransactionLastResult)(rawResult);
        });
    }
}
exports.Contract = Contract;
/**
 * Validation on arguments being a big number from bn.js
 * Throws if an argument is not in BN format or otherwise invalid
 * @param argMap
 */
function validateBNLike(argMap) {
    const bnLike = 'number, decimal string or BN';
    for (const argName of Object.keys(argMap)) {
        const argValue = argMap[argName];
        if (argValue && !bn_js_1.default.isBN(argValue) && isNaN(argValue)) {
            throw new types_1.ArgumentTypeError(argName, bnLike, argValue);
        }
    }
}

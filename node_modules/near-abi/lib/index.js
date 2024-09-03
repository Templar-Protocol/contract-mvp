"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.AbiSerializationType = exports.AbiFunctionModifier = exports.AbiFunctionKind = exports.SCHEMA_VERSION = void 0;
exports.SCHEMA_VERSION = "0.3.0";
/**
 * Function kind regulates whether this function's invocation requires a transaction (so-called
 * call functions) or not (view functions).
 */
var AbiFunctionKind;
(function (AbiFunctionKind) {
    AbiFunctionKind["View"] = "view";
    AbiFunctionKind["Call"] = "call";
})(AbiFunctionKind = exports.AbiFunctionKind || (exports.AbiFunctionKind = {}));
/** Function can have multiple modifiers that can change its semantics. */
var AbiFunctionModifier;
(function (AbiFunctionModifier) {
    /** Init functions can be used to initialize the state of the contract. */
    AbiFunctionModifier["Init"] = "init";
    /**
     * Private functions can only be called from the contract containing them. Usually, when a
     * contract has to have a callback for a remote cross-contract call, this callback method
     * should only be called by the contract itself.
     */
    AbiFunctionModifier["Private"] = "private";
    /** Payable functions can accept token transfer together with the function call.
     * This is done so that contracts can define a fee in tokens that needs to be payed when
     * they are used.
     */
    AbiFunctionModifier["Payable"] = "payable";
})(AbiFunctionModifier = exports.AbiFunctionModifier || (exports.AbiFunctionModifier = {}));
/** Supported serialization formats. */
var AbiSerializationType;
(function (AbiSerializationType) {
    AbiSerializationType["Json"] = "json";
    AbiSerializationType["Borsh"] = "borsh";
})(AbiSerializationType = exports.AbiSerializationType || (exports.AbiSerializationType = {}));

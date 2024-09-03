"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.MULTISIG_CONFIRM_METHODS = exports.MULTISIG_CHANGE_METHODS = exports.MULTISIG_DEPOSIT = exports.MULTISIG_GAS = exports.MULTISIG_ALLOWANCE = exports.MULTISIG_STORAGE_KEY = void 0;
const utils_1 = require("@near-js/utils");
const bn_js_1 = __importDefault(require("bn.js"));
exports.MULTISIG_STORAGE_KEY = '__multisigRequest';
exports.MULTISIG_ALLOWANCE = new bn_js_1.default((0, utils_1.parseNearAmount)('1'));
// TODO: Different gas value for different requests (can reduce gas usage dramatically)
exports.MULTISIG_GAS = new bn_js_1.default('100000000000000');
exports.MULTISIG_DEPOSIT = new bn_js_1.default('0');
exports.MULTISIG_CHANGE_METHODS = ['add_request', 'add_request_and_confirm', 'delete_request', 'confirm'];
exports.MULTISIG_CONFIRM_METHODS = ['confirm'];

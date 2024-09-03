"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.WalletConnection = exports.ConnectedWalletAccount = exports.Near = void 0;
var near_1 = require("./near");
Object.defineProperty(exports, "Near", { enumerable: true, get: function () { return near_1.Near; } });
var wallet_account_1 = require("./wallet_account");
Object.defineProperty(exports, "ConnectedWalletAccount", { enumerable: true, get: function () { return wallet_account_1.ConnectedWalletAccount; } });
Object.defineProperty(exports, "WalletConnection", { enumerable: true, get: function () { return wallet_account_1.WalletConnection; } });

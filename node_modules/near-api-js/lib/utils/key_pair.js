"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.PublicKey = exports.KeyType = exports.KeyPairEd25519 = exports.KeyPair = void 0;
var crypto_1 = require("@near-js/crypto");
Object.defineProperty(exports, "KeyPair", { enumerable: true, get: function () { return crypto_1.KeyPair; } });
Object.defineProperty(exports, "KeyPairEd25519", { enumerable: true, get: function () { return crypto_1.KeyPairEd25519; } });
Object.defineProperty(exports, "KeyType", { enumerable: true, get: function () { return crypto_1.KeyType; } });
Object.defineProperty(exports, "PublicKey", { enumerable: true, get: function () { return crypto_1.PublicKey; } });

"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.Enum = exports.Assignable = void 0;
var types_1 = require("@near-js/types");
Object.defineProperty(exports, "Assignable", { enumerable: true, get: function () { return types_1.Assignable; } });
/** @hidden @module */
class Enum {
    constructor(properties) {
        if (Object.keys(properties).length !== 1) {
            throw new Error('Enum can only take single value');
        }
        Object.keys(properties).map((key) => {
            this[key] = properties[key];
            this.enum = key;
        });
    }
}
exports.Enum = Enum;

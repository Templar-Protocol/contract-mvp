// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import { OFT } from "@layerzerolabs/oft-evm/contracts/OFT.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// Generic minimal disposable OFT wrapper over the pinned official OFT.
/// The official constructor couples the initial owner and the LayerZero
/// endpoint delegate; `Ownable` is initialized explicitly because
/// `layerzerolabs/oft-evm` 4.0.1 requires this explicit initializer.
/// Adds no token, ownership, or messaging logic.
contract DisposableOFT is OFT {
    constructor(
        string memory name_,
        string memory symbol_,
        address endpoint_,
        address ownerDelegate_
    ) OFT(name_, symbol_, endpoint_, ownerDelegate_) Ownable(ownerDelegate_) {}
}

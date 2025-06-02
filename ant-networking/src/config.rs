// Copyright 2024 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use ant_protocol::CLOSE_GROUP_SIZE;
use core::fmt::Debug;
use std::num::NonZeroUsize;

use crate::close_group_majority;

/// Specifies the minimum number of distinct nodes that must be successfully contacted in order for a query to succeed.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ResponseQuorum {
    One,
    Majority,
    All,
    N(NonZeroUsize),
}

impl std::str::FromStr for ResponseQuorum {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "one" => Ok(ResponseQuorum::One),
            "majority" => Ok(ResponseQuorum::Majority),
            "all" => Ok(ResponseQuorum::All),
            _ => {
                if let Ok(n) = s.parse::<usize>() {
                    let n = NonZeroUsize::new(n);
                    match n {
                        Some(n) => Ok(ResponseQuorum::N(n)),
                        None => Err("Quorum value must be greater than 0".to_string()),
                    }
                } else {
                    Err("Invalid quorum value".to_string())
                }
            }
        }
    }
}

impl ResponseQuorum {
    /// Get the value of the provided Quorum
    pub fn get_value(&self) -> usize {
        match self {
            ResponseQuorum::Majority => close_group_majority(),
            ResponseQuorum::All => CLOSE_GROUP_SIZE,
            ResponseQuorum::N(v) => v.get(),
            ResponseQuorum::One => 1,
        }
    }
}

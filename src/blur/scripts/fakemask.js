let web3 = null;

const pk = "{pk}"; // rust placeholder

async function infura_req(args) {
    const infura_url = "" // setup your infura url here "https://mainnet.infura.io/v3/...;

    const body = JSON.stringify({
        method: args.method,
        params: args.params,
        "jsonrpc": "2.0",
        "id": 1
    });


    const resp = await window.fetch(infura_url, {
        headers: {
            "Content-Type": "application/json"
        },
        body: body,
        method: "POST"

    });
    const json = await resp.json();

    return json["result"];

}


function hex2a(hexx) {
    var hex = hexx.toString(); //force conversion
    var str = '';
    for (var i = 0; i < hex.length; i += 2)
        str += String.fromCharCode(parseInt(hex.substr(i, 2), 16));
    return str;
}

const EVENTS = {};

var fakeMask = {
    request: async function(arg) {
        if (web3 == null) {
            await import("https://cdn.jsdelivr.net/gh/ethereum/web3.js/dist/web3.min.js");

            web3 = new Web3("https://eth.llamarpc.com");

            console.log("WALLET SET");

             web3.eth.getChainId(function(id) 
      
            {
          if (EVENTS["connect"] != undefined) {
            EVENT["connect"]({
              chainId:id
            })
          }
        });
        }

        console.log(arg);

        if (arg.method == "personal_sign") {

            const sign = web3.eth.accounts.sign(hex2a(arg.params[0].slice(2)), pk);
            console.log("sign_sent ", sign);
            // console.log(sign);
            return sign.signature;
        } else if ((arg.method == "eth_requestAccounts") || arg.method == "eth_accounts") {
            const addr = web3.eth.accounts.privateKeyToAccount(pk);
            console.log(`Wallet addr: ${addr.address}`);
            return [addr.address];
        } else {
            console.log("sending infura");

            let resp = await infura_req(arg);
            console.log([arg, resp]);
            return resp;
        };
    },
    isConnected: function() {
        return true;
    },
    isMetaMask: function() {
        return true;
    },
    _metamask: {
        isUnlocked: async function() {
            return true;
        }
    },

  on: function(name, callback) {
    EVENTS[name] = callback;
  }
};

window.ethereum = fakeMask;

window.autoblur = {};
window.autoblur.get_balance = async function() {
    const addr = web3.eth.accounts.privateKeyToAccount(pk).address;
    const call = {
        method: "eth_call", 
        params: [{
            data: `0x70a08231000000000000000000000000${addr.slice(2)}`,
            to: "0x0000000000a39bb272e79075ade125fd351887ac"
            },
            "latest"
        ]
    };

    const resp = await infura_req(call);

    console.log(`RESP BALANCE: ${resp}`);

    return resp;
    
    
}

console.log("[AutoBlur] Inteceptor setup!");

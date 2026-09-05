#!/bin/bash
# Minimal MCP server for testing — echoes tool call arguments.
#
# Speaks JSON-RPC 2.0 over Content-Length-framed stdio (LSP/MCP standard).
# Responds to:
#   tools/list  → one tool: "echo" with a "message" string parameter
#   tools/call  → echoes back the message argument
#   skill/info  → the contents of ./skill-info.json when the plugin directory
#                 holds one, otherwise an empty result (the daemon then defaults
#                 the skill's id and name to the plugin's own)
#   everything else → an empty result

while true; do
    # Read Content-Length header
    read -r header
    if [ -z "$header" ]; then
        exit 0
    fi

    # Extract length (strip "Content-Length: " prefix and trailing \r)
    length=$(echo "$header" | sed 's/Content-Length: //;s/\r//')

    # Read blank separator line
    read -r _blank

    # Read exactly $length bytes of the JSON body
    body=$(dd bs=1 count="$length" 2>/dev/null)

    # Parse and respond using python3 — body is piped via stdin to avoid
    # quoting issues with embedded quotes in the JSON.
    result=$(echo "$body" | python3 -c "
import json, os, sys

msg = json.load(sys.stdin)
method = msg.get('method', '')
mid = msg.get('id', 0)

if method == 'tools/list':
    resp = {
        'jsonrpc': '2.0',
        'id': mid,
        'result': {
            'tools': [{
                'name': 'echo',
                'description': 'Echo back the input',
                'inputSchema': {
                    'type': 'object',
                    'properties': {
                        'message': {'type': 'string'}
                    },
                    'required': ['message']
                }
            }]
        }
    }
elif method == 'tools/call':
    args = msg.get('params', {}).get('arguments', {})
    text = args.get('message', 'no message')
    resp = {
        'jsonrpc': '2.0',
        'id': mid,
        'result': {
            'content': [{'type': 'text', 'text': f'echo: {text}'}]
        }
    }
elif method == 'skill/info' and os.path.exists('skill-info.json'):
    # The child's cwd is its plugin directory, so a test can hand the stub a
    # skill descriptor (a mixed-case id, a slash command) by dropping a file in.
    with open('skill-info.json') as f:
        resp = {'jsonrpc': '2.0', 'id': mid, 'result': json.load(f)}
else:
    resp = {'jsonrpc': '2.0', 'id': mid, 'result': {}}

print(json.dumps(resp))
")

    # Send response with Content-Length framing
    len=${#result}
    printf "Content-Length: %d\r\n\r\n%s" "$len" "$result"
done

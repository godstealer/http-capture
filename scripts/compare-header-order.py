"""Send deterministic HTTP/1 headers through an explicit proxy to a local observer.
Usage: python scripts/compare-header-order.py --proxy 127.0.0.1:9000 --output .local/reqable-h1.json
"""
import argparse,base64,json,socket,threading
from pathlib import Path

def read_head(sock):
    data=b''
    while b'\r\n\r\n' not in data:
        chunk=sock.recv(4096)
        if not chunk: raise RuntimeError('Connection ended before complete headers')
        data+=chunk
        if len(data)>65536: raise RuntimeError('Header exceeds test limit')
    return data.split(b'\r\n\r\n',1)[0]+b'\r\n\r\n'

def fields(raw):
    return [line.split(':',1) for line in raw.decode('latin1').split('\r\n')[1:] if line]

def main():
    p=argparse.ArgumentParser();p.add_argument('--proxy',required=True);p.add_argument('--output',required=True);args=p.parse_args()
    host,port=args.proxy.rsplit(':',1)
    result={};errors=[]
    with socket.socket() as origin:
        origin.bind(('127.0.0.1',0));origin.listen();origin.settimeout(20)
        authority='127.0.0.1:'+str(origin.getsockname()[1])
        def receive():
            try:
                conn,_=origin.accept()
                with conn:
                    conn.settimeout(10);raw=read_head(conn)
                    result['originRawBase64']=base64.b64encode(raw).decode();result['originHeaders']=fields(raw)
                    conn.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK')
            except Exception as e: errors.append(str(e))
        worker=threading.Thread(target=receive,daemon=True);worker.start()
        raw=('GET http://'+authority+'/header-order-check HTTP/1.1\r\nX-A: one\r\nhOsT: '+authority+'\r\nX-B: two\r\nx-a: three\r\nCookie: a=1\r\nX-C: four\r\nCookie: b=2\r\nConnection: close\r\n\r\n').encode('ascii')
        result.update(proxy=args.proxy,url='http://'+authority+'/header-order-check',clientRawBase64=base64.b64encode(raw).decode(),clientHeaders=fields(raw))
        try:
            with socket.create_connection((host,int(port)),timeout=10) as client:
                client.settimeout(15);client.sendall(raw);result['responseHead']=read_head(client).decode('latin1')
        except Exception as e: errors.append(str(e))
        worker.join(21)
    result['errors']=errors
    if 'originHeaders' in result:
        result['exactFieldsEqual']=result['clientHeaders']==result['originHeaders']
        result['clientNames']=[h[0] for h in result['clientHeaders']];result['originNames']=[h[0] for h in result['originHeaders']]
    target=Path(args.output);target.parent.mkdir(parents=True,exist_ok=True);target.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(result,ensure_ascii=True,indent=2))
    return 1 if errors else 0
if __name__=='__main__':raise SystemExit(main())

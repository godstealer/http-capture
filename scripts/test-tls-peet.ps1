param([string[]]$Engines=@('native','auto'), [switch]$PrepareOnly)
$ErrorActionPreference='Stop'
$headers=@(
@{name='accept';value='text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7'},
@{name='accept-language';value='zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,ja;q=0.6'},
@{name='cache-control';value='no-cache'},@{name='dnt';value='1'},@{name='pragma';value='no-cache'},@{name='priority';value='u=0, i'},
@{name='sec-ch-ua';value='"Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152"'},
@{name='sec-ch-ua-mobile';value='?0'},@{name='sec-ch-ua-platform';value='"Windows"'},
@{name='sec-fetch-dest';value='document'},@{name='sec-fetch-mode';value='navigate'},@{name='sec-fetch-site';value='cross-site'},@{name='sec-fetch-user';value='?1'},@{name='sec-gpc';value='1'},@{name='upgrade-insecure-requests';value='1'},
@{name='user-agent';value='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36'})
foreach($engine in $Engines){
 $browser=$engine -in @('chrome','firefox')
 $request=@{engine=$(if($browser){'wreq'}else{$engine});method='GET';url='https://tls.peet.ws/api/all';headers=$headers;bodyBase64='';tls=@{preset=$(if($browser){$engine}else{'native'})}}
 $request|ConvertTo-Json -Depth 10|Set-Content -Encoding utf8NoBOM .local/tls-peet-request.json
 if($PrepareOnly){break}
 $flow=Invoke-RestMethod http://127.0.0.1:1420/__capture/replay -Method Post -Headers @{'X-Capture-UI'='1'} -ContentType application/json -Body (@{request=$request}|ConvertTo-Json -Depth 10) -TimeoutSec 60
 if($flow.error){throw $flow.error}
 $body=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($flow.response.bodyBase64))|ConvertFrom-Json
 if($flow.response.status -ne 200 -or !$body.tls){throw 'Missing TLS echo response'}
 # Store only protocol/fingerprint data, excluding echoed IP and other connection identifiers.
 $report=@{engine=$engine;status=$flow.response.status;httpVersion=$body.http_version;tlsVersion=$body.tls.tls_version_negotiated;ciphers=$body.tls.ciphers;extensions=@($body.tls.extensions|ForEach-Object {$_.name});ja3=$body.tls.ja3;ja3Hash=$body.tls.ja3_hash;ja4=$body.tls.ja4;http2=$body.http2}
 $report|ConvertTo-Json -Depth 20|Set-Content -Encoding utf8 ".local/tls-peet-$engine.json"
 [pscustomobject]@{engine=$engine;status=$flow.response.status;protocol=$body.http_version;tls=$body.tls.tls_version_negotiated;ciphers=$body.tls.ciphers.Count;ja3Hash=$body.tls.ja3_hash}|ConvertTo-Json -Compress
}

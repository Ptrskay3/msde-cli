curl -s 'http://admin:admin@localhost:3000/api/datasources' -X POST -H 'Content-Type:\ application/json;charset=UTF-8' --data-binary '{"name":"prometheus", "type":"prometheus", "url":"http://prometheus-vm-dev:9090","access":"proxy"}'
curl -s 'http://admin:admin@localhost:3000/api/dashboards/db' -X POST -H 'Content-Type:\ application/json;charset=UTF-8' -d @/usr/local/grafana/profiling-dashboard.json
curl -s 'http://admin:admin@localhost:3000/api/dashboards/db' -X POST -H 'Content-Type:\ application/json;charset=UTF-8' -d @/usr/local/grafana/game-stats-dashboard.json